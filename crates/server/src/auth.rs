//! Online mode: the key exchange and the question to Mojang's session server
//! whether the player really owns the account (`ServerLoginPacketListenerImpl
//! .handleKey`).

use std::time::Duration;

use anyhow::{Context, bail};
use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::rsa::{KeySize, Pkcs1PrivateDecryptingKey, PrivateDecryptingKey};
use ferrocraft_protocol::packets::login::{GameProfile, ProfileProperty};
use serde::Deserialize;
use uuid::Uuid;

/// Where vanilla asks, from Mojang's discovery document
/// (`session.endpoints.verify`).
pub const SESSION_SERVER: &str = "https://sessionserver.mojang.com/session/minecraft/hasJoined";
/// How long Mojang gets to answer.
const SESSION_TIMEOUT: Duration = Duration::from_secs(10);

/// The server's RSA key pair, new for every start like vanilla's.
pub struct Keys {
    private: Pkcs1PrivateDecryptingKey,
    /// DER-encoded `SubjectPublicKeyInfo`, as sent to the client.
    pub public_der: Vec<u8>,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keys").finish_non_exhaustive()
    }
}

impl Keys {
    /// 2048 bits: vanilla uses 1024 (`Crypt.generateKeyPair`), which AWS-LC
    /// no longer generates; the client takes any size.
    pub fn generate() -> anyhow::Result<Self> {
        let private = PrivateDecryptingKey::generate(KeySize::Rsa2048)
            .map_err(|_| anyhow::anyhow!("generating the RSA key"))?;
        let public_der = private
            .public_key()
            .as_der()
            .map_err(|_| anyhow::anyhow!("encoding the public key"))?
            .as_ref()
            .to_vec();
        let private = Pkcs1PrivateDecryptingKey::new(private)
            .map_err(|_| anyhow::anyhow!("preparing the RSA key"))?;
        Ok(Self {
            private,
            public_der,
        })
    }

    /// Decrypts something the client encrypted with the public key, with
    /// PKCS #1 v1.5 as the protocol prescribes. AWS-LC does this in
    /// constant time, so timing reveals nothing about the padding.
    pub fn decrypt(&self, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut plain = vec![0; self.private.min_output_size()];
        let len = self
            .private
            .decrypt(data, &mut plain)
            .map_err(|_| anyhow::anyhow!("decrypting with the server key"))?
            .len();
        plain.truncate(len);
        Ok(plain)
    }
}

#[derive(Deserialize)]
struct SessionProfile {
    id: String,
    name: String,
    #[serde(default)]
    properties: Vec<SessionProperty>,
}

#[derive(Deserialize)]
struct SessionProperty {
    name: String,
    value: String,
    signature: Option<String>,
}

/// Asks the session server whether `name` joined with this server hash.
/// `Ok(None)` means it did not: the client is not who it claims to be.
pub async fn has_joined(
    http: &reqwest::Client,
    url: &str,
    name: &str,
    server_hash: &str,
) -> anyhow::Result<Option<GameProfile>> {
    let response = http
        .get(url)
        .query(&[("username", name), ("serverId", server_hash)])
        .timeout(SESSION_TIMEOUT)
        .send()
        .await
        .context("asking the session server")?;
    match response.status().as_u16() {
        200 => {}
        // No such join.
        204 => return Ok(None),
        status => bail!("session server answered {status}"),
    }
    let body = response
        .bytes()
        .await
        .context("reading the session server's answer")?;
    let profile: SessionProfile =
        serde_json::from_slice(&body).context("parsing the session server's answer")?;
    Ok(Some(GameProfile {
        uuid: Uuid::parse_str(&profile.id).context("profile id")?,
        name: profile.name,
        properties: profile
            .properties
            .into_iter()
            .map(|p| ProfileProperty {
                name: p.name,
                value: p.value,
                signature: p.signature,
            })
            .collect(),
    }))
}
