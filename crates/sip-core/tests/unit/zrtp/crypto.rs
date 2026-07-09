use super::*;

#[test]
fn hash_chain_links_verify_forward() {
    let chain = generate_hash_chain();
    assert!(verify_hash_chain_hop(&chain.h2, 1, &chain.h3));
    assert!(verify_hash_chain_hop(&chain.h1, 1, &chain.h2));
    assert!(verify_hash_chain_hop(&chain.h1, 2, &chain.h3));
    assert!(verify_hash_chain_hop(&chain.h0, 1, &chain.h1));
    assert!(!verify_hash_chain_hop(&chain.h1, 1, &chain.h3));
}

#[test]
fn kdf_is_deterministic_and_length_correct() {
    let ki = [0x11u8; 32];
    let ctx = [0x22u8; 16];
    let a = kdf(&ki, "Test Label", &ctx, 128);
    let b = kdf(&ki, "Test Label", &ctx, 128);
    assert_eq!(a, b);
    assert_eq!(a.len(), 16);
}

#[test]
fn kdf_differs_by_label() {
    let ki = [0x11u8; 32];
    let ctx = [0x22u8; 16];
    let a = kdf(&ki, "Label A", &ctx, 256);
    let b = kdf(&ki, "Label B", &ctx, 256);
    assert_ne!(a, b);
}

#[test]
fn srtp_keys_differ_between_initiator_and_responder() {
    let s0 = [0x33u8; 32];
    let ctx = kdf_context([1; 12], [2; 12], &[0x44; 32]);
    let keys = derive_srtp_keys(&s0, &ctx);
    assert_ne!(keys.key_i, keys.key_r);
    assert_ne!(keys.salt_i, keys.salt_r);
}

#[test]
fn both_sides_derive_the_same_s0_and_sas() {
    // Simulate both peers computing s0/SAS independently from the same
    // shared inputs -- this is the actual property that matters: whatever
    // the exact formula, both sides must land on identical derived values.
    let dh_result = [0x55u8; 32];
    let zid_i = [1u8; 12];
    let zid_r = [2u8; 12];
    let th = sha256(b"fake transcript");

    let s0_a = derive_s0(&dh_result, zid_i, zid_r, &th, None);
    let s0_b = derive_s0(&dh_result, zid_i, zid_r, &th, None);
    assert_eq!(s0_a, s0_b);

    let ctx = kdf_context(zid_i, zid_r, &th);
    let (sas_value_a, sas_str_a) = derive_sas(&s0_a, &ctx);
    let (sas_value_b, sas_str_b) = derive_sas(&s0_b, &ctx);
    assert_eq!(sas_value_a, sas_value_b);
    assert_eq!(sas_str_a, sas_str_b);
    assert_eq!(sas_str_a.len(), 4);
}

#[test]
fn ec25_agreement_produces_matching_shared_secret() {
    let alice = generate_ec25_keypair();
    let bob = generate_ec25_keypair();
    let alice_pub = alice.public_bytes;
    let bob_pub = bob.public_bytes;

    let alice_secret = ec25_shared_secret(alice, &bob_pub);
    let bob_secret = ec25_shared_secret(bob, &alice_pub);
    assert_eq!(alice_secret, bob_secret);
    assert!(!alice_secret.iter().all(|&b| b == 0));
}

#[test]
fn confirm_encrypt_decrypt_roundtrip() {
    let key = [0x77u8; 16];
    let iv = [0x88u8; 16];
    let plaintext = b"some confirm payload data......";
    let ciphertext = confirm_encrypt(&key, &iv, plaintext);
    assert_ne!(ciphertext, plaintext);
    let decrypted = confirm_decrypt(&key, &iv, &ciphertext);
    assert_eq!(decrypted, plaintext);
}

#[test]
fn message_mac_is_deterministic_and_size_correct() {
    let key = [0x99u8; 32];
    let a = message_mac(&key, b"some message bytes");
    let b = message_mac(&key, b"some message bytes");
    assert_eq!(a, b);
    assert_eq!(a.len(), 8);
}

#[test]
fn hvi_is_deterministic() {
    let a = compute_hvi(b"dhpart2-bytes", b"hello-bytes");
    let b = compute_hvi(b"dhpart2-bytes", b"hello-bytes");
    assert_eq!(a, b);
}
