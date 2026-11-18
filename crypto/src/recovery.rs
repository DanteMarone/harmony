//! The recovery code: the root of a person's identity on Harmony.
//!
//! A recovery code is 160 random bits, shown once as 32 letters and digits.
//! Two things are derived from it (HKDF-SHA256):
//!
//! * an **identity key** (Ed25519). Its public half is published. It signs
//!   each of the person's devices, so friends who have checked the identity
//!   key once can tell that person's real devices from an impostor's.
//! * a **backup key** (AES-256-GCM) that encrypts the person's message
//!   history before it is stored on the server, so a new device can read
//!   it back.
//!
//! The code is random rather than chosen because the server stores the
//! encrypted backup: a chosen passphrase could be guessed by whoever holds
//! the server's disk, a random 160-bit code cannot.

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use sha2::{Digest, Sha256, Sha512};

use crate::device::Error;

type Result<T> = std::result::Result<T, Error>;

const SEED_BYTES: usize = 20;
/// Crockford's base32: no I, L, O or U, so codes survive being read aloud.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const NONCE_BYTES: usize = 12;

pub const IDENTITY_KEY_BYTES: usize = 32;
pub const SIGNATURE_BYTES: usize = 64;
pub const BACKUP_KEY_BYTES: usize = 32;

fn random(buffer: &mut [u8]) -> Result<()> {
    getrandom::getrandom(buffer).map_err(|e| Error(format!("no random source: {e}")))
}

pub struct Recovery {
    seed: [u8; SEED_BYTES],
}

impl Recovery {
    pub fn generate() -> Result<Self> {
        let mut seed = [0u8; SEED_BYTES];
        random(&mut seed)?;
        Ok(Self { seed })
    }

    /// Reads a code as typed by a person: case, spaces and dashes are
    /// ignored, and the look-alikes O, I and L are read as 0, 1 and 1.
    pub fn from_code(code: &str) -> Result<Self> {
        let mut values = Vec::with_capacity(SEED_BYTES * 8 / 5);
        for c in code.chars() {
            let c = match c.to_ascii_uppercase() {
                ' ' | '-' | '\t' | '\n' | '\r' => continue,
                'O' => '0',
                'I' | 'L' => '1',
                other => other,
            };
            let value = ALPHABET
                .iter()
                .position(|&a| a as char == c)
                .ok_or_else(|| Error(format!("a recovery code cannot contain \"{c}\"")))?;
            values.push(value as u8);
        }
        if values.len() != SEED_BYTES * 8 / 5 {
            return Err(Error(format!(
                "a recovery code has {} letters and digits, this has {}",
                SEED_BYTES * 8 / 5,
                values.len()
            )));
        }
        let mut seed = [0u8; SEED_BYTES];
        for (chunk, out) in values.chunks(8).zip(seed.chunks_mut(5)) {
            let bits = chunk.iter().fold(0u64, |acc, &v| (acc << 5) | v as u64);
            out.copy_from_slice(&bits.to_be_bytes()[3..]);
        }
        Ok(Self { seed })
    }

    /// The code in eight groups of four, for example `7KQ2-…`.
    pub fn code(&self) -> String {
        let mut out = String::new();
        for chunk in self.seed.chunks(5) {
            let mut wide = [0u8; 8];
            wide[3..].copy_from_slice(chunk);
            let bits = u64::from_be_bytes(wide);
            for i in (0..8).rev() {
                if out.len() % 5 == 4 {
                    out.push('-');
                }
                out.push(ALPHABET[((bits >> (i * 5)) & 31) as usize] as char);
            }
        }
        out
    }

    fn derive(&self, label: &[u8]) -> [u8; 32] {
        let mut out = [0u8; 32];
        Hkdf::<Sha256>::new(Some(b"harmony recovery v1"), &self.seed)
            .expand(label, &mut out)
            .expect("32 bytes is a valid HKDF output length");
        out
    }

    fn signing_key(&self) -> SigningKey {
        SigningKey::from_bytes(&self.derive(b"identity key"))
    }

    /// The public identity key that friends verify.
    pub fn identity_key(&self) -> Vec<u8> {
        self.signing_key().verifying_key().to_bytes().to_vec()
    }

    /// States "this device, with this key, is mine".
    pub fn sign_device(&self, device_identity: &str, device_key: &[u8]) -> Vec<u8> {
        self.signing_key()
            .sign(&device_statement(device_identity, device_key))
            .to_bytes()
            .to_vec()
    }

    /// The key that encrypts this person's history backup.
    pub fn backup_key(&self) -> Vec<u8> {
        self.derive(b"backup key").to_vec()
    }
}

fn device_statement(device_identity: &str, device_key: &[u8]) -> Vec<u8> {
    let mut statement = b"harmony device v1\0".to_vec();
    statement.extend_from_slice(&(device_identity.len() as u32).to_be_bytes());
    statement.extend_from_slice(device_identity.as_bytes());
    statement.extend_from_slice(device_key);
    statement
}

/// Whether `signature` is the owner of `identity_key` vouching for a device.
pub fn verify_device(
    identity_key: &[u8],
    device_identity: &str,
    device_key: &[u8],
    signature: &[u8],
) -> bool {
    let Ok(key) = <[u8; IDENTITY_KEY_BYTES]>::try_from(identity_key) else {
        return false;
    };
    let Ok(key) = VerifyingKey::from_bytes(&key) else {
        return false;
    };
    let Ok(signature) = Signature::from_slice(signature) else {
        return false;
    };
    key.verify(&device_statement(device_identity, device_key), &signature)
        .is_ok()
}

/// A number two people compare (in person, or over a call) to be sure
/// nobody sits between them. Both see the same sixty digits, whichever of
/// them asks. `a` and `b` are each a user id with that user's identity key.
pub fn safety_number(a: (u64, &[u8]), b: (u64, &[u8])) -> String {
    let (first, second) = if a.0 <= b.0 { (a, b) } else { (b, a) };
    let mut hash = Sha512::new();
    hash.update(b"harmony safety number v1\0");
    for (user, key) in [first, second] {
        hash.update(user.to_be_bytes());
        hash.update((key.len() as u32).to_be_bytes());
        hash.update(key);
    }
    let digest = hash.finalize();
    digest[..60]
        .chunks(5)
        .map(|chunk| {
            let mut wide = [0u8; 8];
            wide[3..].copy_from_slice(chunk);
            format!("{:05}", u64::from_be_bytes(wide) % 100_000)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn cipher(key: &[u8]) -> Result<Aes256Gcm> {
    Aes256Gcm::new_from_slice(key).map_err(|_| Error("backup key has the wrong length".into()))
}

/// Encrypts one backup record. `context` says what the record is (whose,
/// which chat, which message); a record moved anywhere else fails to open.
pub fn seal(key: &[u8], context: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
    let mut nonce = [0u8; NONCE_BYTES];
    random(&mut nonce)?;
    let mut out = nonce.to_vec();
    let sealed = cipher(key)?
        .encrypt(
            &Nonce::from(nonce),
            Payload {
                msg: plaintext,
                aad: context.as_bytes(),
            },
        )
        .map_err(|_| Error("could not encrypt backup record".into()))?;
    out.extend_from_slice(&sealed);
    Ok(out)
}

pub fn open(key: &[u8], context: &str, sealed: &[u8]) -> Result<Vec<u8>> {
    if sealed.len() < NONCE_BYTES {
        return Err(Error("backup record is cut short".into()));
    }
    let (nonce, body) = sealed.split_at(NONCE_BYTES);
    let nonce: [u8; NONCE_BYTES] = nonce.try_into().expect("split at the nonce length");
    cipher(key)?
        .decrypt(
            &Nonce::from(nonce),
            Payload {
                msg: body,
                aad: context.as_bytes(),
            },
        )
        .map_err(|_| Error("backup record could not be decrypted".into()))
}
