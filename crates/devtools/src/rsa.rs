//! RSA encryption with a public key, PKCS #1 v1.5, as the client side of
//! online mode needs it. Written out here because AWS-LC refuses the 1024-bit
//! keys vanilla servers use. Only public data goes in, so unlike decryption
//! there is no secret for timing to leak.

use num_bigint::BigUint;

pub struct PublicKey {
    modulus: BigUint,
    exponent: BigUint,
    /// Length of the modulus in bytes.
    size: usize,
}

impl PublicKey {
    /// Reads a DER `SubjectPublicKeyInfo` with an RSA key.
    pub fn from_der(der: &[u8]) -> Option<Self> {
        let mut spki = Der(der).sequence()?;
        spki.sequence()?; // algorithm
        let key = spki.bit_string()?;
        let mut key = Der(key).sequence()?;
        let modulus = BigUint::from_bytes_be(key.integer()?);
        let exponent = BigUint::from_bytes_be(key.integer()?);
        let size = modulus.bits().div_ceil(8) as usize;
        Some(Self {
            modulus,
            exponent,
            size,
        })
    }

    /// `0x00 0x02 <random non-zero padding> 0x00 <message>`, raised to the
    /// exponent.
    pub fn encrypt(&self, message: &[u8]) -> Vec<u8> {
        assert!(
            message.len() + 11 <= self.size,
            "message too long for the key"
        );
        let mut block = vec![0u8; self.size];
        block[1] = 2;
        let padding = self.size - message.len() - 3;
        for byte in &mut block[2..2 + padding] {
            *byte = loop {
                let b: u8 = rand::random();
                if b != 0 {
                    break b;
                }
            };
        }
        block[self.size - message.len()..].copy_from_slice(message);
        let encrypted = BigUint::from_bytes_be(&block)
            .modpow(&self.exponent, &self.modulus)
            .to_bytes_be();
        let mut out = vec![0u8; self.size - encrypted.len()];
        out.extend_from_slice(&encrypted);
        out
    }
}

/// Just enough DER to take a public key apart.
struct Der<'a>(&'a [u8]);

impl<'a> Der<'a> {
    fn element(&mut self, tag: u8) -> Option<&'a [u8]> {
        let (&found, rest) = self.0.split_first()?;
        if found != tag {
            return None;
        }
        let (&first, mut rest) = rest.split_first()?;
        let len = if first & 0x80 == 0 {
            usize::from(first)
        } else {
            let (bytes, after) = rest.split_at_checked(usize::from(first & 0x7f))?;
            rest = after;
            bytes.iter().fold(0, |len, &b| len << 8 | usize::from(b))
        };
        let (content, after) = rest.split_at_checked(len)?;
        self.0 = after;
        Some(content)
    }

    fn sequence(&mut self) -> Option<Der<'a>> {
        self.element(0x30).map(Der)
    }

    fn bit_string(&mut self) -> Option<&'a [u8]> {
        // The first byte counts unused bits, always 0 for a key.
        self.element(0x03)?.split_first().map(|(_, bits)| bits)
    }

    fn integer(&mut self) -> Option<&'a [u8]> {
        self.element(0x02)
    }
}
