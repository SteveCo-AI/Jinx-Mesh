use anyhow::{anyhow, bail, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use rand::{rngs::OsRng, RngCore};
use std::io::{Read, Write};
use zeroize::Zeroizing;

pub(crate) type Key = Zeroizing<[u8; 32]>;
pub(crate) const MAX_PLAIN: usize = 64 * 1024 * 1024;
pub(crate) fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    OsRng.fill_bytes(&mut bytes);
    bytes
}
pub(crate) fn derive(password: &str, salt: &[u8]) -> Result<Key> {
    if salt.len() != 16 || password.len() > 1024 {
        bail!("Anmeldung fehlgeschlagen");
    }
    let mut key = Zeroizing::new([0; 32]);
    let params = Params::new(65536, 3, 1, Some(32)).map_err(|_| anyhow!("KDF-Konfiguration"))?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, key.as_mut())
        .map_err(|_| anyhow!("Anmeldung fehlgeschlagen"))?;
    Ok(key)
}
pub(crate) fn seal(key: &Key, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    if plaintext.len() > MAX_PLAIN {
        bail!("Zustand zu groß");
    }
    let mut compressor =
        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    compressor.write_all(plaintext)?;
    let compressed = Zeroizing::new(compressor.finish()?);
    let nonce = random::<24>();
    let cipher =
        XChaCha20Poly1305::new_from_slice(key.as_ref()).map_err(|_| anyhow!("Schlüssel"))?;
    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: &compressed,
                aad,
            },
        )
        .map_err(|_| anyhow!("Verschlüsselung fehlgeschlagen"))?;
    let mut out = nonce.to_vec();
    out.extend(ciphertext);
    Ok(out)
}
pub(crate) fn unseal(key: &Key, blob: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if blob.len() < 40 || blob.len() > MAX_PLAIN + 1024 * 1024 {
        bail!("Anmeldung oder Integritätsprüfung fehlgeschlagen");
    }
    let cipher =
        XChaCha20Poly1305::new_from_slice(key.as_ref()).map_err(|_| anyhow!("Schlüssel"))?;
    let compressed = Zeroizing::new(
        cipher
            .decrypt(
                XNonce::from_slice(&blob[..24]),
                Payload {
                    msg: &blob[24..],
                    aad,
                },
            )
            .map_err(|_| anyhow!("Anmeldung oder Integritätsprüfung fehlgeschlagen"))?,
    );
    let mut plain = Zeroizing::new(Vec::new());
    flate2::read::ZlibDecoder::new(&compressed[..])
        .take(MAX_PLAIN as u64 + 1)
        .read_to_end(&mut plain)?;
    if plain.len() > MAX_PLAIN {
        bail!("Dekompressionslimit");
    }
    Ok(plain)
}
