//! Exercises the encryption layer the way the app uses it: several devices,
//! with this test standing in for the server that relays opaque bytes.

use harmony_crypto::{Device, Processed};

const CHAT: &str = "chat-1";

fn text(processed: Processed) -> (String, String) {
    match processed {
        Processed::Message { sender, content } => (sender, String::from_utf8(content).unwrap()),
        Processed::MembershipChanged => panic!("expected a message"),
    }
}

/// Alice creates a chat and adds the given devices. Returns after everyone joined.
fn chat_with(alice: &Device, others: &[&Device]) {
    alice.create_chat(CHAT).unwrap();
    let add: Vec<_> = others
        .iter()
        .map(|d| {
            (
                d.identity().to_owned(),
                d.key_packages(1, false).unwrap().remove(0),
            )
        })
        .collect();
    let commit = alice.change_members(CHAT, &add, &[]).unwrap();
    alice.confirm_commit(CHAT).unwrap();
    let welcome = commit.welcome.unwrap();
    for device in others {
        assert_eq!(device.join_chat(&welcome).unwrap(), CHAT);
    }
}

#[test]
fn two_devices_exchange_messages() {
    let alice = Device::create("u1:d1").unwrap();
    let bob = Device::create("u2:d2").unwrap();
    chat_with(&alice, &[&bob]);

    let ciphertext = alice.encrypt(CHAT, b"hello bob").unwrap();
    assert!(
        !ciphertext.windows(9).any(|w| w == b"hello bob"),
        "plaintext must not appear on the wire"
    );
    assert_eq!(
        text(bob.process(CHAT, &ciphertext).unwrap()),
        ("u1:d1".into(), "hello bob".into())
    );

    let reply = bob.encrypt(CHAT, b"hi alice").unwrap();
    assert_eq!(
        text(alice.process(CHAT, &reply).unwrap()),
        ("u2:d2".into(), "hi alice".into())
    );

    let mut members = alice.members(CHAT).unwrap();
    members.sort();
    assert_eq!(members, ["u1:d1", "u2:d2"]);
    assert_eq!(alice.epoch(CHAT).unwrap(), bob.epoch(CHAT).unwrap());
}

#[test]
fn a_message_cannot_be_replayed() {
    let alice = Device::create("u1:d1").unwrap();
    let bob = Device::create("u2:d2").unwrap();
    chat_with(&alice, &[&bob]);
    let ciphertext = alice.encrypt(CHAT, b"once").unwrap();
    assert!(bob.process(CHAT, &ciphertext).is_ok());
    assert!(bob.process(CHAT, &ciphertext).is_err());
}

#[test]
fn outsiders_and_removed_members_cannot_read() {
    let alice = Device::create("u1:d1").unwrap();
    let bob = Device::create("u2:d2").unwrap();
    let carol = Device::create("u3:d3").unwrap();
    let eve = Device::create("u9:d9").unwrap();
    chat_with(&alice, &[&bob, &carol]);

    // Eve was never in the chat.
    let ciphertext = alice.encrypt(CHAT, b"secret").unwrap();
    assert!(eve.process(CHAT, &ciphertext).is_err());
    text(bob.process(CHAT, &ciphertext).unwrap());
    text(carol.process(CHAT, &ciphertext).unwrap());

    // Alice removes Carol. Bob applies the change.
    let removal = alice.change_members(CHAT, &[], &["u3:d3".to_owned()]).unwrap();
    assert!(removal.welcome.is_none());
    alice.confirm_commit(CHAT).unwrap();
    assert!(matches!(
        bob.process(CHAT, &removal.commit).unwrap(),
        Processed::MembershipChanged
    ));
    assert_eq!(alice.members(CHAT).unwrap().len(), 2);

    // Carol holds only the old keys: she cannot read what comes next,
    // whether or not she saw the removal.
    let after = alice.encrypt(CHAT, b"carol is gone").unwrap();
    assert!(carol.process(CHAT, &after).is_err());
    let _ = carol.process(CHAT, &removal.commit);
    assert!(carol.process(CHAT, &after).is_err());
    assert_eq!(text(bob.process(CHAT, &after).unwrap()).1, "carol is gone");
}

#[test]
fn a_later_joiner_cannot_read_earlier_messages() {
    let alice = Device::create("u1:d1").unwrap();
    let bob = Device::create("u2:d2").unwrap();
    let dave = Device::create("u4:d4").unwrap();
    chat_with(&alice, &[&bob]);
    let early = alice.encrypt(CHAT, b"before dave").unwrap();
    text(bob.process(CHAT, &early).unwrap());

    let add = [("u4:d4".to_owned(), dave.key_packages(1, false).unwrap().remove(0))];
    let commit = bob.change_members(CHAT, &add, &[]).unwrap();
    bob.confirm_commit(CHAT).unwrap();
    alice.process(CHAT, &commit.commit).unwrap();
    dave.join_chat(&commit.welcome.unwrap()).unwrap();

    assert!(dave.process(CHAT, &early).is_err());
    let late = alice.encrypt(CHAT, b"welcome dave").unwrap();
    assert_eq!(text(dave.process(CHAT, &late).unwrap()).1, "welcome dave");
    text(bob.process(CHAT, &late).unwrap());
}

#[test]
fn a_key_package_under_the_wrong_name_is_refused() {
    let alice = Device::create("u1:d1").unwrap();
    let mallory = Device::create("u66:d66").unwrap();
    alice.create_chat(CHAT).unwrap();
    // The server claims Mallory's key package belongs to Bob's device.
    let add = [(
        "u2:d2".to_owned(),
        mallory.key_packages(1, false).unwrap().remove(0),
    )];
    assert!(alice.change_members(CHAT, &add, &[]).is_err());
    assert_eq!(alice.members(CHAT).unwrap(), ["u1:d1"]);
}

#[test]
fn a_refused_commit_can_be_discarded() {
    let alice = Device::create("u1:d1").unwrap();
    let bob = Device::create("u2:d2").unwrap();
    let carol = Device::create("u3:d3").unwrap();
    chat_with(&alice, &[&bob]);

    // Alice and Bob both try to add Carol; the server accepts Bob's first.
    let for_alice = [(
        "u3:d3".to_owned(),
        carol.key_packages(1, false).unwrap().remove(0),
    )];
    let for_bob = [(
        "u3:d3".to_owned(),
        carol.key_packages(1, false).unwrap().remove(0),
    )];
    let _lost = alice.change_members(CHAT, &for_alice, &[]).unwrap();
    let won = bob.change_members(CHAT, &for_bob, &[]).unwrap();
    bob.confirm_commit(CHAT).unwrap();

    alice.discard_commit(CHAT).unwrap();
    alice.process(CHAT, &won.commit).unwrap();
    carol.join_chat(&won.welcome.unwrap()).unwrap();

    assert_eq!(alice.members(CHAT).unwrap().len(), 3);
    let message = alice.encrypt(CHAT, b"all three").unwrap();
    assert_eq!(text(bob.process(CHAT, &message).unwrap()).1, "all three");
    assert_eq!(text(carol.process(CHAT, &message).unwrap()).1, "all three");
}

#[test]
fn state_survives_a_restart() {
    let alice = Device::create("u1:d1").unwrap();
    let bob = Device::create("u2:d2").unwrap();
    chat_with(&alice, &[&bob]);
    let first = alice.encrypt(CHAT, b"one").unwrap();
    text(bob.process(CHAT, &first).unwrap());

    let saved = bob.export_state().unwrap();
    assert_eq!(saved, bob.export_state().unwrap(), "export must be deterministic");
    drop(bob);
    let bob = Device::restore(&saved).unwrap();
    assert_eq!(bob.identity(), "u2:d2");
    assert!(bob.has_chat(CHAT));

    let second = alice.encrypt(CHAT, b"two").unwrap();
    assert_eq!(text(bob.process(CHAT, &second).unwrap()).1, "two");
    let reply = bob.encrypt(CHAT, b"three").unwrap();
    assert_eq!(text(alice.process(CHAT, &reply).unwrap()).1, "three");

    assert!(Device::restore(b"garbage").is_err());
    assert!(Device::restore(&saved[..saved.len() / 2]).is_err());
}

#[test]
fn last_resort_key_package_can_be_used_twice() {
    let alice = Device::create("u1:d1").unwrap();
    let bob = Device::create("u2:d2").unwrap();
    let package = bob.key_packages(1, true).unwrap().remove(0);
    for chat in ["chat-a", "chat-b"] {
        alice.create_chat(chat).unwrap();
        let commit = alice
            .change_members(chat, &[("u2:d2".to_owned(), package.clone())], &[])
            .unwrap();
        alice.confirm_commit(chat).unwrap();
        assert_eq!(bob.join_chat(&commit.welcome.unwrap()).unwrap(), chat);
    }
}

#[test]
fn members_share_an_exported_secret() {
    let alice = Device::create("u1:d1").unwrap();
    let bob = Device::create("u2:d2").unwrap();
    chat_with(&alice, &[&bob]);
    let a = alice.export_secret(CHAT, "harmony voice", 32).unwrap();
    let b = bob.export_secret(CHAT, "harmony voice", 32).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.len(), 32);
}
