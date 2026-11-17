use std::collections::HashMap;

use openmls::prelude::{tls_codec::*, *};
use openmls_basic_credential::SignatureKeyPair;
use openmls_memory_storage::MemoryStorage;
use openmls_rust_crypto::RustCrypto;
use openmls_traits::OpenMlsProvider;

/// The mandatory-to-implement MLS ciphersuite: X25519, AES-128-GCM, SHA-256, Ed25519.
const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
const STATE_MAGIC: &[u8; 8] = b"HRMNYDV1";

#[derive(Debug)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn err<E: std::fmt::Debug>(context: &'static str) -> impl FnOnce(E) -> Error {
    move |e| Error(format!("{context}: {e:?}"))
}

type Result<T> = std::result::Result<T, Error>;

/// OpenMLS needs a crypto backend and somewhere to keep keys. Keys live in
/// memory and are saved by the app through [`Device::export_state`].
#[derive(Default)]
struct Provider {
    crypto: RustCrypto,
    storage: MemoryStorage,
}

impl OpenMlsProvider for Provider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = MemoryStorage;

    fn storage(&self) -> &MemoryStorage {
        &self.storage
    }
    fn crypto(&self) -> &RustCrypto {
        &self.crypto
    }
    fn rand(&self) -> &RustCrypto {
        &self.crypto
    }
}

/// A change to a chat's membership, ready to send to the server.
pub struct Commit {
    /// For everyone already in the chat.
    pub commit: Vec<u8>,
    /// For the devices being added, if any.
    pub welcome: Option<Vec<u8>>,
}

/// What an incoming message turned out to be.
pub enum Processed {
    /// A chat message: who sent it (their identity string) and its content.
    Message { sender: String, content: Vec<u8> },
    /// The chat's membership or keys changed; the new state is already applied.
    MembershipChanged,
}

/// One device's keys and its view of every chat it is in.
pub struct Device {
    provider: Provider,
    identity: String,
    signer: SignatureKeyPair,
}

impl Device {
    /// Makes a brand-new device with fresh keys. `identity` is the name other
    /// devices will check it against, for example `u12:d34`.
    pub fn create(identity: &str) -> Result<Self> {
        let provider = Provider::default();
        let signer =
            SignatureKeyPair::new(CIPHERSUITE.signature_algorithm()).map_err(err("key generation"))?;
        signer
            .store(provider.storage())
            .map_err(err("storing signature key"))?;
        Ok(Self {
            provider,
            identity: identity.to_owned(),
            signer,
        })
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// The public half of this device's signature key. Friends compare a
    /// fingerprint of it to check nobody is impersonating the device.
    pub fn signature_public_key(&self) -> Vec<u8> {
        self.signer.public().to_vec()
    }

    // -- saving and restoring ------------------------------------------------

    /// Everything needed to bring this device back after a restart. It
    /// contains private keys: store it only on the device itself.
    pub fn export_state(&self) -> Result<Vec<u8>> {
        let values = self
            .provider
            .storage
            .values
            .read()
            .map_err(err("reading key store"))?;
        let mut out = Vec::new();
        out.extend_from_slice(STATE_MAGIC);
        put(&mut out, self.identity.as_bytes());
        put(&mut out, self.signer.public());
        out.extend_from_slice(&(values.len() as u32).to_be_bytes());
        // Sorted so the same state always exports to the same bytes.
        let mut entries: Vec<_> = values.iter().collect();
        entries.sort();
        for (key, value) in entries {
            put(&mut out, key);
            put(&mut out, value);
        }
        Ok(out)
    }

    pub fn restore(state: &[u8]) -> Result<Self> {
        let mut input = state;
        if take_exact(&mut input, STATE_MAGIC.len())? != STATE_MAGIC {
            return Err(Error("saved device state is not in a known format".into()));
        }
        let identity = String::from_utf8(take(&mut input)?.to_vec()).map_err(err("identity"))?;
        let public_key = take(&mut input)?.to_vec();
        let count = u32::from_be_bytes(take_exact(&mut input, 4)?.try_into().unwrap());
        let mut values = HashMap::new();
        for _ in 0..count {
            let key = take(&mut input)?.to_vec();
            let value = take(&mut input)?.to_vec();
            values.insert(key, value);
        }
        let provider = Provider::default();
        *provider
            .storage
            .values
            .write()
            .map_err(err("writing key store"))? = values;
        let signer =
            SignatureKeyPair::read(provider.storage(), &public_key, CIPHERSUITE.signature_algorithm())
                .ok_or_else(|| Error("saved device state has no signature key".into()))?;
        Ok(Self {
            provider,
            identity,
            signer,
        })
    }

    // -- key packages ----------------------------------------------------------

    /// Makes `count` key packages: one-time tokens that let other devices add
    /// this one to a chat while it is offline. A last-resort package may be
    /// reused when the one-time ones run out.
    pub fn key_packages(&self, count: usize, last_resort: bool) -> Result<Vec<Vec<u8>>> {
        (0..count)
            .map(|_| {
                let mut builder = KeyPackage::builder().leaf_node_capabilities(capabilities());
                if last_resort {
                    builder = builder.mark_as_last_resort();
                }
                let bundle = builder
                    .build(CIPHERSUITE, &self.provider, &self.signer, self.credential())
                    .map_err(err("building key package"))?;
                bundle
                    .key_package()
                    .tls_serialize_detached()
                    .map_err(err("encoding key package"))
            })
            .collect()
    }

    // -- chats -----------------------------------------------------------------

    /// Starts a new chat containing only this device.
    pub fn create_chat(&self, chat_id: &str) -> Result<()> {
        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .capabilities(capabilities())
            .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
            .use_ratchet_tree_extension(true)
            .padding_size(64)
            .build();
        MlsGroup::new_with_group_id(
            &self.provider,
            &self.signer,
            &config,
            GroupId::from_slice(chat_id.as_bytes()),
            self.credential(),
        )
        .map_err(err("creating chat"))?;
        Ok(())
    }

    pub fn has_chat(&self, chat_id: &str) -> bool {
        self.group(chat_id).is_ok()
    }

    /// Forgets a chat and its keys, for example after leaving it.
    pub fn delete_chat(&self, chat_id: &str) -> Result<()> {
        let mut group = self.group(chat_id)?;
        group
            .delete(self.provider.storage())
            .map_err(err("deleting chat"))
    }

    /// The chat's epoch: a counter that goes up by one on every membership change.
    pub fn epoch(&self, chat_id: &str) -> Result<u64> {
        Ok(self.group(chat_id)?.epoch().as_u64())
    }

    /// Identities of every device currently in the chat.
    pub fn members(&self, chat_id: &str) -> Result<Vec<String>> {
        self.group(chat_id)?
            .members()
            .map(|m| identity_of(&m.credential))
            .collect()
    }

    /// Adds and removes devices in one step.
    ///
    /// `add` pairs each key package with the identity the server claims it
    /// belongs to. The claim is checked against what the key package itself
    /// says, so the server cannot slip in a device under someone else's name.
    ///
    /// The change is only *proposed* until the server accepts it: call
    /// [`Device::confirm_commit`] on success or [`Device::discard_commit`]
    /// if the server refuses (someone else changed the chat first).
    pub fn change_members(
        &self,
        chat_id: &str,
        add: &[(String, Vec<u8>)],
        remove: &[String],
    ) -> Result<Commit> {
        let mut group = self.group(chat_id)?;

        let mut key_packages = Vec::with_capacity(add.len());
        for (expected_identity, bytes) in add {
            let key_package = KeyPackageIn::tls_deserialize_exact(bytes.as_slice())
                .map_err(err("decoding key package"))?
                .validate(self.provider.crypto(), ProtocolVersion::Mls10)
                .map_err(err("key package failed validation"))?;
            let actual = identity_of(key_package.leaf_node().credential())?;
            if &actual != expected_identity {
                return Err(Error(format!(
                    "key package is for {actual}, but it was presented as {expected_identity}"
                )));
            }
            key_packages.push(key_package);
        }

        let mut leaves = Vec::with_capacity(remove.len());
        for member in group.members() {
            if remove.contains(&identity_of(&member.credential)?) {
                leaves.push(member.index);
            }
        }

        let mut builder = group.commit_builder();
        if !key_packages.is_empty() {
            builder = builder.propose_adds(key_packages);
        }
        if !leaves.is_empty() {
            builder = builder.propose_removals(leaves);
        }
        let bundle = builder
            .load_psks(self.provider.storage())
            .map_err(err("loading keys"))?
            .build(self.provider.rand(), self.provider.crypto(), &self.signer, |_| {
                true
            })
            .map_err(err("building commit"))?
            .stage_commit(&self.provider)
            .map_err(err("staging commit"))?;

        let commit = bundle
            .commit()
            .tls_serialize_detached()
            .map_err(err("encoding commit"))?;
        let welcome = match bundle.welcome() {
            Some(_) => Some(
                bundle
                    .to_welcome_msg()
                    .ok_or_else(|| Error("commit has no welcome".into()))?
                    .tls_serialize_detached()
                    .map_err(err("encoding welcome"))?,
            ),
            None => None,
        };
        Ok(Commit { commit, welcome })
    }

    /// The server accepted our membership change: apply it.
    pub fn confirm_commit(&self, chat_id: &str) -> Result<()> {
        self.group(chat_id)?
            .merge_pending_commit(&self.provider)
            .map_err(err("applying commit"))
    }

    /// The server refused our membership change: drop it.
    pub fn discard_commit(&self, chat_id: &str) -> Result<()> {
        self.group(chat_id)?
            .clear_pending_commit(self.provider.storage())
            .map_err(err("discarding commit"))
    }

    /// Joins a chat this device was added to. Returns the chat id.
    pub fn join_chat(&self, welcome: &[u8]) -> Result<String> {
        let message = MlsMessageIn::tls_deserialize_exact(welcome).map_err(err("decoding welcome"))?;
        let MlsMessageBodyIn::Welcome(welcome) = message.extract() else {
            return Err(Error("expected a welcome message".into()));
        };
        let config = MlsGroupJoinConfig::builder()
            .wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY)
            .use_ratchet_tree_extension(true)
            .padding_size(64)
            .build();
        let group = StagedWelcome::new_from_welcome(&self.provider, &config, welcome, None)
            .map_err(err("reading welcome"))?
            .into_group(&self.provider)
            .map_err(err("joining chat"))?;
        String::from_utf8(group.group_id().as_slice().to_vec()).map_err(err("chat id"))
    }

    /// Encrypts a message for everyone in the chat.
    pub fn encrypt(&self, chat_id: &str, content: &[u8]) -> Result<Vec<u8>> {
        self.group(chat_id)?
            .create_message(&self.provider, &self.signer, content)
            .map_err(err("encrypting message"))?
            .tls_serialize_detached()
            .map_err(err("encoding message"))
    }

    /// Decrypts and applies something received from the chat. Each message
    /// can be processed exactly once: the keys are destroyed after use.
    pub fn process(&self, chat_id: &str, message: &[u8]) -> Result<Processed> {
        let mut group = self.group(chat_id)?;
        let message = MlsMessageIn::tls_deserialize_exact(message)
            .map_err(err("decoding message"))?
            .try_into_protocol_message()
            .map_err(err("not a chat message"))?;
        if message.group_id().as_slice() != chat_id.as_bytes() {
            return Err(Error("message belongs to a different chat".into()));
        }
        let processed = group
            .process_message(&self.provider, message)
            .map_err(err("processing message"))?;
        let sender = identity_of(processed.credential())?;
        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(application) => Ok(Processed::Message {
                sender,
                content: application.into_bytes(),
            }),
            ProcessedMessageContent::StagedCommitMessage(staged) => {
                group
                    .merge_staged_commit(&self.provider, *staged)
                    .map_err(err("applying commit"))?;
                Ok(Processed::MembershipChanged)
            }
            // Harmony never sends standalone proposals; ignore any we are shown.
            _ => Err(Error("unexpected message type".into())),
        }
    }

    /// Derives a secret every current member can compute and nobody else
    /// can, for example to key an encrypted voice call.
    pub fn export_secret(&self, chat_id: &str, label: &str, length: usize) -> Result<Vec<u8>> {
        self.group(chat_id)?
            .export_secret(self.provider.crypto(), label, &[], length)
            .map_err(err("exporting secret"))
    }

    // -- internals -------------------------------------------------------------

    fn credential(&self) -> CredentialWithKey {
        CredentialWithKey {
            credential: BasicCredential::new(self.identity.as_bytes().to_vec()).into(),
            signature_key: self.signer.public().into(),
        }
    }

    fn group(&self, chat_id: &str) -> Result<MlsGroup> {
        MlsGroup::load(self.provider.storage(), &GroupId::from_slice(chat_id.as_bytes()))
            .map_err(err("loading chat"))?
            .ok_or_else(|| Error(format!("this device is not in chat {chat_id}")))
    }
}

/// Every device declares support for last-resort key packages, so one can
/// always be added to a chat even after its one-time key packages run out.
fn capabilities() -> Capabilities {
    Capabilities::new(None, None, Some(&[ExtensionType::LastResort]), None, None)
}

fn identity_of(credential: &Credential) -> Result<String> {
    let basic = BasicCredential::try_from(credential.clone()).map_err(err("reading credential"))?;
    String::from_utf8(basic.identity().to_vec()).map_err(err("credential identity"))
}

fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn take_exact<'a>(input: &mut &'a [u8], length: usize) -> Result<&'a [u8]> {
    if input.len() < length {
        return Err(Error("saved device state is cut short".into()));
    }
    let (head, tail) = input.split_at(length);
    *input = tail;
    Ok(head)
}

fn take<'a>(input: &mut &'a [u8]) -> Result<&'a [u8]> {
    let length = u32::from_be_bytes(take_exact(input, 4)?.try_into().unwrap()) as usize;
    take_exact(input, length)
}
