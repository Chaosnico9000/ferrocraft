//! Online mode's transport encryption: AES-128 in CFB8 mode with the shared
//! secret as both key and IV, applied to every byte after the key exchange
//! (`Crypt.getCipher`, `CipherEncoder`, `CipherDecoder`).

use aes::Aes128;
use aes::cipher::generic_array::GenericArray;
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use sha1::{Digest, Sha1};

/// One connection's two cipher streams.
pub struct Cipher {
    encrypt: cfb8::Encryptor<Aes128>,
    decrypt: cfb8::Decryptor<Aes128>,
}

impl Cipher {
    pub fn new(secret: &[u8; 16]) -> Self {
        Self {
            encrypt: cfb8::Encryptor::new(secret.into(), secret.into()),
            decrypt: cfb8::Decryptor::new(secret.into(), secret.into()),
        }
    }

    /// Encrypts outgoing bytes in place; the stream continues across calls.
    pub fn encrypt(&mut self, bytes: &mut [u8]) {
        for byte in bytes.chunks_mut(1) {
            self.encrypt
                .encrypt_block_mut(GenericArray::from_mut_slice(byte));
        }
    }

    /// Decrypts incoming bytes in place.
    pub fn decrypt(&mut self, bytes: &mut [u8]) {
        for byte in bytes.chunks_mut(1) {
            self.decrypt
                .decrypt_block_mut(GenericArray::from_mut_slice(byte));
        }
    }
}

/// The server id the client and the server both report to Mojang
/// (`Crypt.digestData` printed as a signed `BigInteger` in hex).
pub fn server_hash(server_id: &str, secret: &[u8], public_key: &[u8]) -> String {
    let mut sha = Sha1::new();
    sha.update(server_id.as_bytes());
    sha.update(secret);
    sha.update(public_key);
    signed_hex(sha.finalize().into())
}

/// A big-endian two's complement number in hex, minus sign and no leading
/// zeros, like Java's `new BigInteger(bytes).toString(16)`.
fn signed_hex(mut digest: [u8; 20]) -> String {
    let negative = digest[0] & 0x80 != 0;
    if negative {
        // Two's complement: invert, then add one.
        let mut carry = true;
        for byte in digest.iter_mut().rev() {
            *byte = !*byte;
            if carry {
                let (sum, overflow) = byte.overflowing_add(1);
                *byte = sum;
                carry = overflow;
            }
        }
    }
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let trimmed = hex.trim_start_matches('0');
    let digits = if trimmed.is_empty() { "0" } else { trimmed };
    if negative {
        format!("-{digits}")
    } else {
        digits.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha1_hex(name: &str) -> String {
        signed_hex(Sha1::digest(name.as_bytes()).into())
    }

    #[test]
    fn hashes_match_the_known_examples() {
        // The examples everyone checks Minecraft's hash against.
        assert_eq!(
            sha1_hex("Notch"),
            "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48"
        );
        assert_eq!(
            sha1_hex("jeb_"),
            "-7c9d5b0044c130109a5d7b5fb5c317c02b4e28c1"
        );
        assert_eq!(sha1_hex("simon"), "88e16a1019277b15d58faf0541e11910eb756f6");
    }

    #[test]
    fn cipher_streams_across_calls() {
        let secret = [7u8; 16];
        let mut sender = Cipher::new(&secret);
        let mut receiver = Cipher::new(&secret);
        let message = b"Hallo, verschluesselte Welt!".to_vec();
        let mut wire = message.clone();
        sender.encrypt(&mut wire[..5]);
        sender.encrypt(&mut wire[5..]);
        assert_ne!(wire, message);
        // Split differently on the way in: a stream cipher does not care.
        receiver.decrypt(&mut wire[..11]);
        receiver.decrypt(&mut wire[11..]);
        assert_eq!(wire, message);
    }
}
