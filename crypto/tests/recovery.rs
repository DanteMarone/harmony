use harmony_crypto::{Device, recovery};
use recovery::Recovery;

#[test]
fn code_round_trips_and_forgives_typing() {
    let original = Recovery::generate().unwrap();
    let code = original.code();
    assert_eq!(code.len(), 39, "eight groups of four with dashes: {code}");
    assert!(
        code.split('-')
            .all(|g| g.len() == 4 && g.chars().all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()))
    );

    let sloppy = code
        .to_lowercase()
        .replace('-', " ")
        .replace('0', "o")
        .replace('1', "l");
    let typed = Recovery::from_code(&sloppy).unwrap();
    assert_eq!(typed.code(), code);
    assert_eq!(typed.identity_key(), original.identity_key());
    assert_eq!(typed.backup_key(), original.backup_key());

    assert!(Recovery::from_code("ABCD").is_err());
    assert!(Recovery::from_code(&code.replace('-', "!")).is_err());
}

#[test]
fn different_codes_give_unrelated_keys() {
    let a = Recovery::generate().unwrap();
    let b = Recovery::generate().unwrap();
    assert_ne!(a.code(), b.code());
    assert_ne!(a.identity_key(), b.identity_key());
    assert_ne!(a.backup_key(), b.backup_key());
    assert_ne!(a.backup_key(), a.identity_key());
}

#[test]
fn device_signature_only_fits_the_device_it_was_made_for() {
    let owner = Recovery::generate().unwrap();
    let device = Device::create("u1:d1").unwrap();
    let key = device.signature_public_key();
    let signature = owner.sign_device("u1:d1", &key);

    assert!(recovery::verify_device(
        &owner.identity_key(),
        "u1:d1",
        &key,
        &signature
    ));
    // Another device name, another key, or another owner: all refused.
    assert!(!recovery::verify_device(
        &owner.identity_key(),
        "u1:d2",
        &key,
        &signature
    ));
    let other = Device::create("u1:d1").unwrap().signature_public_key();
    assert!(!recovery::verify_device(
        &owner.identity_key(),
        "u1:d1",
        &other,
        &signature
    ));
    let stranger = Recovery::generate().unwrap();
    assert!(!recovery::verify_device(
        &stranger.identity_key(),
        "u1:d1",
        &key,
        &signature
    ));
    // Garbage never panics.
    assert!(!recovery::verify_device(&[1, 2, 3], "u1:d1", &key, &signature));
    assert!(!recovery::verify_device(
        &owner.identity_key(),
        "u1:d1",
        &key,
        &[0; 5]
    ));
}

#[test]
fn chat_members_can_be_checked_against_identity_keys() {
    let owner = Recovery::generate().unwrap();
    let alice = Device::create("u1:d1").unwrap();
    let bob = Device::create("u2:d2").unwrap();
    alice.create_chat("c").unwrap();
    let package = bob.key_packages(1, false).unwrap().remove(0);
    alice
        .change_members("c", &[("u2:d2".into(), package)], &[])
        .unwrap();
    alice.confirm_commit("c").unwrap();

    let members = alice.member_keys("c").unwrap();
    assert_eq!(members.len(), 2);
    let (identity, key) = members.iter().find(|(i, _)| i == "u1:d1").unwrap();
    assert_eq!(key, &alice.signature_public_key());
    let signature = owner.sign_device(identity, key);
    assert!(recovery::verify_device(
        &owner.identity_key(),
        identity,
        key,
        &signature
    ));
    let (_, bobs_key) = members.iter().find(|(i, _)| i == "u2:d2").unwrap();
    assert_eq!(bobs_key, &bob.signature_public_key());
}

#[test]
fn safety_number_is_the_same_from_both_sides() {
    let a = Recovery::generate().unwrap().identity_key();
    let b = Recovery::generate().unwrap().identity_key();
    let number = recovery::safety_number((7, &a), (3, &b));
    assert_eq!(number, recovery::safety_number((3, &b), (7, &a)));
    assert_eq!(number.split(' ').count(), 12);
    assert!(
        number
            .split(' ')
            .all(|g| g.len() == 5 && g.chars().all(|c| c.is_ascii_digit()))
    );

    // A different key for either person changes it.
    let c = Recovery::generate().unwrap().identity_key();
    assert_ne!(number, recovery::safety_number((7, &c), (3, &b)));
    assert_ne!(number, recovery::safety_number((7, &a), (4, &b)));
}

#[test]
fn backup_records_open_only_with_the_right_key_and_place() {
    let owner = Recovery::generate().unwrap();
    let key = owner.backup_key();
    let sealed = recovery::seal(&key, "u1/chat/5", b"hello").unwrap();
    assert_ne!(sealed, recovery::seal(&key, "u1/chat/5", b"hello").unwrap());
    assert!(!sealed.windows(5).any(|w| w == b"hello"));
    assert_eq!(recovery::open(&key, "u1/chat/5", &sealed).unwrap(), b"hello");

    assert!(recovery::open(&key, "u1/chat/6", &sealed).is_err());
    let other = Recovery::generate().unwrap().backup_key();
    assert!(recovery::open(&other, "u1/chat/5", &sealed).is_err());
    let mut tampered = sealed.clone();
    *tampered.last_mut().unwrap() ^= 1;
    assert!(recovery::open(&key, "u1/chat/5", &tampered).is_err());
    assert!(recovery::open(&key, "u1/chat/5", &[1, 2, 3]).is_err());
}
