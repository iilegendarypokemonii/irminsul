use crate::{CaptureMode, capture_mode::select_backend, compatibility::CompatibilityCapture};
use anyhow::{Context, Result};
use futures::{StreamExt, stream::BoxStream};
use pktmon::filter::{PktMonFilter, TransportProtocol};

pub(crate) enum PacketSource {
    Modern(BoxStream<'static, pktmon::Packet>),
    Compatibility(CompatibilityCapture),
}

impl PacketSource {
    pub(crate) fn open(mode: CaptureMode) -> Result<Self> {
        select_backend(mode, Self::modern, || {
            CompatibilityCapture::open().map(Self::Compatibility)
        })
    }

    fn modern() -> Result<Self> {
        let mut capture = pktmon::Capture::isolated()?;
        for port in [22101u16, 22102] {
            capture.add_filter(PktMonFilter {
                name: format!("Irminsul-{port}"),
                transport_protocol: Some(TransportProtocol::UDP),
                port: port.into(),
                ..Default::default()
            })?;
        }
        Ok(Self::Modern(capture.stream()?.boxed()))
    }

    pub(crate) fn ready_label(&self) -> &'static [u8] {
        match self {
            Self::Modern(_) => b"modern",
            Self::Compatibility(_) => b"compatibility",
        }
    }

    pub(crate) async fn next_packet(&mut self) -> Result<Vec<u8>> {
        match self {
            Self::Modern(packets) => Ok(packets
                .next()
                .await
                .context("Packet capture ended unexpectedly.")?
                .payload
                .to_vec()
                .clone()),
            Self::Compatibility(capture) => capture.next_packet().await,
        }
    }
}
