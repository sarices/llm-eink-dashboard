use anyhow::{bail, Context, Result};

const SERVICE: &str = "com.local.llm-eink-dashboard";
const KEYCHAIN_TARGET: &str = "User";

fn entry(reference: &str) -> Result<keyring::Entry> {
    keyring::Entry::new_with_target(KEYCHAIN_TARGET, SERVICE, reference)
        .context("create macOS Keychain entry")
}

/// A reference is a stable opaque Keychain account name, never a token value.
pub fn validate_secret_reference(reference: &str) -> Result<()> {
    if reference.trim().is_empty() {
        bail!("secret reference is required")
    }
    if reference.contains("sk-")
        || reference.len() > 128
        || reference.chars().any(char::is_whitespace)
    {
        bail!("store an opaque Keychain reference, not the API key")
    }
    Ok(())
}

pub fn save_secret(reference: &str, value: &str) -> Result<()> {
    validate_secret_reference(reference)?;
    if value.trim().is_empty() {
        bail!("secret value is required")
    }
    let entry = entry(reference)?;
    entry
        .set_password(value)
        .context("store credential in macOS Keychain")?;
    let stored = entry
        .get_password()
        .context("verify credential in macOS Keychain after saving")?;
    if stored != value {
        bail!("macOS Keychain 保存校验失败；请解锁钥匙串后重试")
    }
    Ok(())
}

pub fn load_secret(reference: &str) -> Result<String> {
    validate_secret_reference(reference)?;
    let entry = entry(reference)?;
    match entry.get_password() {
        Ok(value) => Ok(value),
        Err(keyring::Error::NoEntry) => {
            bail!("macOS Keychain 中未找到该凭据；请在“数据源”中编辑此数据源并重新保存 API Key")
        }
        Err(error) => Err(error).context("read credential from macOS Keychain"),
    }
}

pub fn delete_secret(reference: &str) -> Result<()> {
    validate_secret_reference(reference)?;
    match entry(reference)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_token_as_reference() {
        assert!(validate_secret_reference("sk-secret").is_err());
        assert!(validate_secret_reference("deepseek.personal").is_ok());
    }
}
