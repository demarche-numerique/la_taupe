use p256::ecdsa::{signature::Verifier, Signature, VerifyingKey};

use super::certificate_store::certificate;

pub fn check(
    payload_str: &str,
    signature_str: &str,
    autorite_certification: &str,
    identifiant_du_certificat: &str,
) {
    let payload = payload_str.as_bytes();
    let signature = to_signature(signature_str);
    let verifying_key = fetch_verifying_key(autorite_certification, identifiant_du_certificat);

    verifying_key.verify(payload, &signature).unwrap();
}

fn base32_str_to_bytes(base32_str: &str) -> Vec<u8> {
    base32::decode(base32::Alphabet::Rfc4648 { padding: true }, base32_str).unwrap()
}

fn to_signature(signature: &str) -> Signature {
    let signature_bytes: &[u8] = &base32_str_to_bytes(signature);
    Signature::from_slice(signature_bytes).unwrap()
}

fn fetch_verifying_key(
    autorite_certification: &str,
    identifiant_du_certificat: &str,
) -> VerifyingKey {
    let certificate = certificate(autorite_certification, identifiant_du_certificat);

    let key = certificate
        .tbs_certificate()
        .subject_public_key_info()
        .subject_public_key
        .raw_bytes();

    VerifyingKey::from_sec1_bytes(key).unwrap()
}
