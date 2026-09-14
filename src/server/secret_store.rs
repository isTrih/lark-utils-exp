use crate::xingtu::XingtuSession;
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use anyhow::{Context, anyhow};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use std::env;

const SESSION_KEY_ENV: &str = "XINGTU_SESSION_ENCRYPTION_KEY";
const SESSION_KEY_ID_ENV: &str = "XINGTU_SESSION_ENCRYPTION_KEY_ID";
const PROJECT_FEISHU_KEY_ENV: &str = "LARK_PROJECT_CREDENTIAL_ENCRYPTION_KEY";
const PROJECT_FEISHU_KEY_ID_ENV: &str = "LARK_PROJECT_CREDENTIAL_ENCRYPTION_KEY_ID";
const FALLBACK_KEY_ENV: &str = "API_DATA_ENCRYPTION_KEY";
const FALLBACK_KEY_ID_ENV: &str = "API_DATA_ENCRYPTION_KEY_ID";
const DEFAULT_KEY_ID: &str = "primary";
const ENVELOPE_VERSION: u8 = 1;
const NONCE_LENGTH: usize = 12;

#[derive(Clone)]
pub struct SessionCipher {
    key: [u8; 32],
    key_id: String,
}

/// 项目飞书应用密钥加密器。
///
/// 推荐使用独立密钥；为保证旧部署平滑升级，未配置时依次回退到 API 数据保护密钥
/// 和星图登录态密钥。密钥只从运行环境读取，不进入数据库。
#[derive(Clone)]
pub struct ProjectFeishuCredentialCipher {
    key: [u8; 32],
    key_id: String,
}

impl SessionCipher {
    /// 登录态密钥优先独立配置；未配置时可复用 API 数据保护密钥。
    pub fn from_env() -> anyhow::Result<Self> {
        let encoded = env_value(SESSION_KEY_ENV)
            .or_else(|| env_value(FALLBACK_KEY_ENV))
            .ok_or_else(|| {
                anyhow!(
                    "缺少 XINGTU_SESSION_ENCRYPTION_KEY；也未配置可回退使用的 API_DATA_ENCRYPTION_KEY"
                )
            })?;
        let decoded = STANDARD
            .decode(&encoded)
            .context("XINGTU_SESSION_ENCRYPTION_KEY 必须是标准 Base64")?;
        let key: [u8; 32] = decoded
            .try_into()
            .map_err(|_| anyhow!("XINGTU_SESSION_ENCRYPTION_KEY 解码后必须恰好为 32 字节"))?;
        if STANDARD.encode(key) != encoded {
            return Err(anyhow!(
                "XINGTU_SESSION_ENCRYPTION_KEY 必须使用规范标准 Base64"
            ));
        }
        let key_id = env_value(SESSION_KEY_ID_ENV)
            .or_else(|| env_value(FALLBACK_KEY_ID_ENV))
            .unwrap_or_else(|| DEFAULT_KEY_ID.to_owned());
        Ok(Self { key, key_id })
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn encrypt_session(
        &self,
        account_id: &str,
        session: &XingtuSession,
    ) -> anyhow::Result<Vec<u8>> {
        let plaintext = serde_json::to_vec(session).context("序列化星图登录态失败")?;
        let mut nonce = [0_u8; NONCE_LENGTH];
        getrandom::fill(&mut nonce).context("生成登录态加密 nonce 失败")?;
        self.encrypt_with_nonce(account_id, &plaintext, &nonce)
    }

    fn encrypt_with_nonce(
        &self,
        account_id: &str,
        plaintext: &[u8],
        nonce: &[u8],
    ) -> anyhow::Result<Vec<u8>> {
        if nonce.len() != NONCE_LENGTH {
            return Err(anyhow!("登录态加密 nonce 长度无效"));
        }
        let cipher = Aes256Gcm::new_from_slice(&self.key).context("初始化登录态加密器失败")?;
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: plaintext,
                    aad: account_id.as_bytes(),
                },
            )
            .map_err(|_| anyhow!("加密星图登录态失败"))?;
        let mut envelope = Vec::with_capacity(1 + NONCE_LENGTH + ciphertext.len());
        envelope.push(ENVELOPE_VERSION);
        envelope.extend_from_slice(nonce);
        envelope.extend_from_slice(&ciphertext);
        Ok(envelope)
    }

    pub fn decrypt_session(
        &self,
        account_id: &str,
        envelope: &[u8],
    ) -> anyhow::Result<XingtuSession> {
        if envelope.len() <= 1 + NONCE_LENGTH + 16 || envelope[0] != ENVELOPE_VERSION {
            return Err(anyhow!("星图登录态密文信封无效或版本不受支持"));
        }
        let nonce = &envelope[1..1 + NONCE_LENGTH];
        let ciphertext = &envelope[1 + NONCE_LENGTH..];
        let cipher = Aes256Gcm::new_from_slice(&self.key).context("初始化登录态解密器失败")?;
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: account_id.as_bytes(),
                },
            )
            .map_err(|_| anyhow!("星图登录态解密或完整性校验失败"))?;
        serde_json::from_slice(&plaintext).context("解析解密后的星图登录态失败")
    }
}

impl ProjectFeishuCredentialCipher {
    pub fn from_env() -> anyhow::Result<Self> {
        let (encoded, source_name) = env_value(PROJECT_FEISHU_KEY_ENV)
            .map(|value| (value, PROJECT_FEISHU_KEY_ENV))
            .or_else(|| env_value(FALLBACK_KEY_ENV).map(|value| (value, FALLBACK_KEY_ENV)))
            .or_else(|| env_value(SESSION_KEY_ENV).map(|value| (value, SESSION_KEY_ENV)))
            .ok_or_else(|| {
                anyhow!(
                    "缺少 LARK_PROJECT_CREDENTIAL_ENCRYPTION_KEY；也未配置可回退使用的 API_DATA_ENCRYPTION_KEY 或 XINGTU_SESSION_ENCRYPTION_KEY"
                )
            })?;
        let key = decode_key(&encoded, source_name)?;
        let key_id = match source_name {
            PROJECT_FEISHU_KEY_ENV => env_value(PROJECT_FEISHU_KEY_ID_ENV),
            FALLBACK_KEY_ENV => env_value(FALLBACK_KEY_ID_ENV),
            SESSION_KEY_ENV => env_value(SESSION_KEY_ID_ENV),
            _ => None,
        }
        .unwrap_or_else(|| DEFAULT_KEY_ID.to_owned());
        Ok(Self { key, key_id })
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn encrypt_app_secret(
        &self,
        feishu_app_id: i64,
        app_id: &str,
        app_secret: &str,
    ) -> anyhow::Result<Vec<u8>> {
        let mut nonce = [0_u8; NONCE_LENGTH];
        getrandom::fill(&mut nonce).context("生成项目飞书密钥加密 nonce 失败")?;
        encrypt_envelope(
            &self.key,
            &project_feishu_aad(feishu_app_id, app_id),
            app_secret.as_bytes(),
            &nonce,
            "项目飞书 APP_SECRET",
        )
    }

    pub fn decrypt_app_secret(
        &self,
        feishu_app_id: i64,
        app_id: &str,
        envelope: &[u8],
    ) -> anyhow::Result<String> {
        let plaintext = decrypt_envelope(
            &self.key,
            &project_feishu_aad(feishu_app_id, app_id),
            envelope,
            "项目飞书 APP_SECRET",
        )?;
        String::from_utf8(plaintext).context("解密后的项目飞书 APP_SECRET 不是有效 UTF-8")
    }
}

fn decode_key(encoded: &str, source_name: &str) -> anyhow::Result<[u8; 32]> {
    let decoded = STANDARD
        .decode(encoded)
        .with_context(|| format!("{source_name} 必须是标准 Base64"))?;
    let key: [u8; 32] = decoded
        .try_into()
        .map_err(|_| anyhow!("{source_name} 解码后必须恰好为 32 字节"))?;
    if STANDARD.encode(key) != encoded {
        return Err(anyhow!("{source_name} 必须使用规范标准 Base64"));
    }
    Ok(key)
}

fn project_feishu_aad(feishu_app_id: i64, app_id: &str) -> Vec<u8> {
    format!("lark-app:{feishu_app_id}:{}", app_id.trim()).into_bytes()
}

fn encrypt_envelope(
    key: &[u8; 32],
    aad: &[u8],
    plaintext: &[u8],
    nonce: &[u8],
    subject: &str,
) -> anyhow::Result<Vec<u8>> {
    if nonce.len() != NONCE_LENGTH {
        return Err(anyhow!("{subject} 加密 nonce 长度无效"));
    }
    let cipher =
        Aes256Gcm::new_from_slice(key).with_context(|| format!("初始化{subject}加密器失败"))?;
    let ciphertext = cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| anyhow!("加密{subject}失败"))?;
    let mut envelope = Vec::with_capacity(1 + NONCE_LENGTH + ciphertext.len());
    envelope.push(ENVELOPE_VERSION);
    envelope.extend_from_slice(nonce);
    envelope.extend_from_slice(&ciphertext);
    Ok(envelope)
}

fn decrypt_envelope(
    key: &[u8; 32],
    aad: &[u8],
    envelope: &[u8],
    subject: &str,
) -> anyhow::Result<Vec<u8>> {
    if envelope.len() <= 1 + NONCE_LENGTH + 16 || envelope[0] != ENVELOPE_VERSION {
        return Err(anyhow!("{subject} 密文信封无效或版本不受支持"));
    }
    let nonce = &envelope[1..1 + NONCE_LENGTH];
    let ciphertext = &envelope[1 + NONCE_LENGTH..];
    let cipher =
        Aes256Gcm::new_from_slice(key).with_context(|| format!("初始化{subject}解密器失败"))?;
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| anyhow!("{subject} 解密或完整性校验失败"))
}

fn env_value(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cipher() -> SessionCipher {
        SessionCipher {
            key: [0x31; 32],
            key_id: "test".to_owned(),
        }
    }

    #[test]
    fn encrypted_session_round_trips_and_rejects_tampering() {
        let session = XingtuSession {
            cookie: "cookie-value".to_owned(),
            csrf_token: "csrf-value".to_owned(),
            session_key: Some("session-key".to_owned()),
            user_agent: Some("test-agent".to_owned()),
            extra_headers: HashMap::from([("x-test".to_owned(), "value".to_owned())]),
        };
        let nonce = [0x22; NONCE_LENGTH];
        let plaintext = serde_json::to_vec(&session).unwrap();
        let mut envelope = cipher()
            .encrypt_with_nonce("account-a", &plaintext, &nonce)
            .unwrap();
        let restored = cipher().decrypt_session("account-a", &envelope).unwrap();
        assert_eq!(restored.cookie, session.cookie);
        assert_eq!(restored.csrf_token, session.csrf_token);
        assert_eq!(restored.extra_headers, session.extra_headers);
        assert!(cipher().decrypt_session("account-b", &envelope).is_err());

        let last = envelope.len() - 1;
        envelope[last] ^= 1;
        assert!(cipher().decrypt_session("account-a", &envelope).is_err());
    }

    #[test]
    fn project_feishu_secret_is_bound_to_project_and_app() {
        let cipher = ProjectFeishuCredentialCipher {
            key: [0x42; 32],
            key_id: "test".to_owned(),
        };
        let nonce = [0x18; NONCE_LENGTH];
        let encrypted = encrypt_envelope(
            &cipher.key,
            &project_feishu_aad(7, "cli_test"),
            b"secret-value",
            &nonce,
            "项目飞书 APP_SECRET",
        )
        .unwrap();

        assert_eq!(
            cipher
                .decrypt_app_secret(7, "cli_test", &encrypted)
                .unwrap(),
            "secret-value"
        );
        assert!(
            cipher
                .decrypt_app_secret(8, "cli_test", &encrypted)
                .is_err()
        );
        assert!(
            cipher
                .decrypt_app_secret(7, "cli_other", &encrypted)
                .is_err()
        );
    }
}
