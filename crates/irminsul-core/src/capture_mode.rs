use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CaptureMode {
    #[default]
    Auto,
    Compatibility,
}

impl CaptureMode {
    pub(crate) fn argument(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Compatibility => "compatibility",
        }
    }
}

impl std::str::FromStr for CaptureMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "compatibility" => Ok(Self::Compatibility),
            _ => bail!("Unknown capture method."),
        }
    }
}

/// Try the private Packet Monitor implementation first. Compatibility capture
/// has no dependency on Packet Monitor and never uses its global legacy state.
pub(crate) fn select_backend<T>(
    mode: CaptureMode,
    modern: impl FnOnce() -> Result<T>,
    compatibility: impl FnOnce() -> Result<T>,
) -> Result<T> {
    if mode == CaptureMode::Compatibility {
        return compatibility();
    }
    match modern() {
        Ok(backend) => Ok(backend),
        Err(modern_error) => compatibility().map_err(|compatibility_error| {
            anyhow::anyhow!(
                "Neither capture method could start. Compatibility: {compatibility_error:#}. Packet Monitor: {modern_error:#}"
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_uses_modern_when_available() -> Result<()> {
        assert_eq!(
            select_backend(CaptureMode::Auto, || Ok(1), || panic!("unneeded fallback"))?,
            1
        );
        Ok(())
    }

    #[test]
    fn automatic_falls_back_when_private_api_is_unavailable() -> Result<()> {
        assert_eq!(
            select_backend(CaptureMode::Auto, || bail!("API unavailable"), || Ok(2))?,
            2
        );
        Ok(())
    }

    #[test]
    fn compatibility_never_opens_packet_monitor() -> Result<()> {
        assert_eq!(
            select_backend(
                CaptureMode::Compatibility,
                || panic!("must not touch Packet Monitor"),
                || Ok(2)
            )?,
            2
        );
        Ok(())
    }

    #[test]
    fn both_failures_remain_actionable() {
        let error = select_backend::<()>(
            CaptureMode::Auto,
            || bail!("API unavailable"),
            || bail!("No active IPv4 connection"),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("API unavailable"));
        assert!(error.contains("No active IPv4 connection"));
        assert!("unknown".parse::<CaptureMode>().is_err());
        for mode in [CaptureMode::Auto, CaptureMode::Compatibility] {
            assert_eq!(mode.argument().parse::<CaptureMode>().unwrap(), mode);
        }
    }
}
