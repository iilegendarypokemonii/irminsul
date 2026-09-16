//! Windows 10-compatible receive-only capture using Winsock SIO_RCVALL.
//! No driver installation, Packet Monitor mutation, or promiscuous NIC mode.
use etherparse::{NetSlice, SlicedPacket, TransportSlice};

/// Filter before crossing the elevated-helper boundary. The existing decoder
/// accepts Ethernet frames, so wrap the complete IPv4 datagram in a local header.
pub(crate) fn ethernet_frame(ip: &[u8]) -> Option<Vec<u8>> {
    let packet = SlicedPacket::from_ip(ip).ok()?;
    let NetSlice::Ipv4(header) = packet.net? else {
        return None;
    };
    let TransportSlice::Udp(udp) = packet.transport? else {
        return None;
    };
    if ![udp.source_port(), udp.destination_port()]
        .iter()
        .any(|port| [22101, 22102].contains(port))
    {
        return None;
    }
    let length = usize::from(header.header().total_len());
    let mut frame = Vec::with_capacity(14 + length);
    frame.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x08, 0]);
    frame.extend_from_slice(ip.get(..length)?);
    Some(frame)
}

#[cfg(windows)]
pub(crate) use network::CompatibilityCapture;

#[cfg(windows)]
mod network {
    use super::ethernet_frame;
    use anyhow::{Context, Result, bail, ensure};
    use socket2::{Domain, Protocol, Socket, Type};
    use std::{
        collections::VecDeque,
        io::Read,
        net::{Ipv4Addr, SocketAddrV4},
        os::windows::io::AsRawSocket,
        time::Duration,
    };
    use windows::Win32::{
        Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA, NO_ERROR},
        NetworkManagement::{IpHelper::*, Ndis::IfOperStatusUp},
        Networking::WinSock::*,
    };

    pub(crate) struct CompatibilityCapture {
        sockets: Vec<Socket>,
        buffer: Vec<u8>,
        pending: VecDeque<Vec<u8>>,
        pending_bytes: usize,
        tick: tokio::time::Interval,
    }

    impl CompatibilityCapture {
        pub(crate) fn open() -> Result<Self> {
            let addresses = active_addresses()?;
            ensure!(
                !addresses.is_empty(),
                "No active IPv4 network connection. Connect to the network, then start capture again."
            );
            // Every active interface must be covered: silently omitting a VPN or
            // Wi-Fi adapter could report success while missing the game's login.
            let sockets = addresses
                .into_iter()
                .map(open_socket)
                .collect::<Result<_>>()?;
            let mut tick = tokio::time::interval(Duration::from_millis(5));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            Ok(Self {
                sockets,
                buffer: vec![0; 65_535],
                pending: VecDeque::new(),
                pending_bytes: 0,
                tick,
            })
        }

        pub(crate) async fn next_packet(&mut self) -> Result<Vec<u8>> {
            loop {
                if let Some(packet) = self.pending.pop_front() {
                    self.pending_bytes -= packet.len();
                    return Ok(packet);
                }
                self.tick.tick().await;
                for socket in &self.sockets {
                    drain_socket(
                        socket,
                        &mut self.buffer,
                        &mut self.pending,
                        &mut self.pending_bytes,
                    )?;
                }
            }
        }
    }

    fn drain_socket(
        socket: &Socket,
        buffer: &mut [u8],
        pending: &mut VecDeque<Vec<u8>>,
        pending_bytes: &mut usize,
    ) -> Result<()> {
        // Bound each poll so stop/disconnect/deadline can always be observed.
        for _ in 0..256 {
            let length = match (&*socket).read(buffer) {
                Ok(length) => length,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error).context("Network capture interrupted. Stop capture and start it again after reconnecting."),
            };
            if let Some(frame) = ethernet_frame(&buffer[..length]) {
                ensure!(
                    pending.len() < 4096 && *pending_bytes + frame.len() <= 4 * 1024 * 1024,
                    "Capture queue is full. Restart capture and log in again."
                );
                *pending_bytes += frame.len();
                pending.push_back(frame);
            }
        }
        Ok(())
    }

    fn open_socket(address: Ipv4Addr) -> Result<Socket> {
        let socket = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::from(IPPROTO_IP.0)))
            .context("Compatibility capture requires Windows administrator permission.")?;
        socket.set_recv_buffer_size(4 * 1024 * 1024)?;
        socket.bind(&SocketAddrV4::new(address, 0).into())?;
        // Unlike RCVALL_ON, IPLEVEL does not put the network adapter in
        // promiscuous mode. Closing this socket releases the receive request.
        let level = RCVALL_IPLEVEL.0;
        let mut returned = 0;
        let result = unsafe {
            WSAIoctl(
                SOCKET(socket.as_raw_socket() as usize),
                SIO_RCVALL,
                Some((&level as *const i32).cast()),
                size_of::<i32>() as u32,
                None,
                0,
                &mut returned,
                None,
                None,
            )
        };
        if result == SOCKET_ERROR {
            return Err(std::io::Error::from_raw_os_error(unsafe { WSAGetLastError() }.0))
                .context("Windows could not enable compatibility capture. Check network/VPN software and try again.");
        }
        socket.set_nonblocking(true)?;
        Ok(socket)
    }

    fn active_addresses() -> Result<Vec<Ipv4Addr>> {
        let mut size = 15_000u32;
        for _ in 0..3 {
            ensure!(
                size <= 1_048_576,
                "Network adapter information exceeded its limit."
            );
            // u64 storage provides the alignment required by the Windows structs.
            let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
            let first = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
            let result = unsafe {
                GetAdaptersAddresses(
                    AF_INET.0 as u32,
                    GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                    None,
                    Some(first),
                    &mut size,
                )
            };
            if result == ERROR_BUFFER_OVERFLOW.0 {
                continue;
            }
            if result == ERROR_NO_DATA.0 {
                return Ok(Vec::new());
            }
            ensure!(
                result == NO_ERROR.0,
                "Could not list network adapters: Windows error {result}."
            );
            // All linked structs and socket addresses belong to buffer and are
            // traversed only while that allocation remains alive and unmoved.
            let mut addresses = unsafe { collect_addresses(first) };
            addresses.sort_unstable();
            addresses.dedup();
            return Ok(addresses);
        }
        bail!("Network configuration is changing. Wait for it to settle, then restart capture.")
    }

    unsafe fn collect_addresses(mut adapter: *const IP_ADAPTER_ADDRESSES_LH) -> Vec<Ipv4Addr> {
        let mut addresses = Vec::new();
        while let Some(current) = unsafe { adapter.as_ref() } {
            if current.OperStatus == IfOperStatusUp {
                addresses.extend(unsafe { unicast_addresses(current.FirstUnicastAddress) });
            }
            adapter = current.Next;
        }
        addresses
    }

    unsafe fn unicast_addresses(
        mut address: *const IP_ADAPTER_UNICAST_ADDRESS_LH,
    ) -> Vec<Ipv4Addr> {
        let mut addresses = Vec::new();
        while let Some(current) = unsafe { address.as_ref() } {
            if let Some(socket) = unsafe { current.Address.lpSockaddr.as_ref() } {
                if socket.sa_family == AF_INET
                    && current.Address.iSockaddrLength as usize >= size_of::<SOCKADDR_IN>()
                {
                    let ipv4 = unsafe { &*current.Address.lpSockaddr.cast::<SOCKADDR_IN>() };
                    let ip = Ipv4Addr::from(unsafe { ipv4.sin_addr.S_un.S_addr }.to_ne_bytes());
                    if !ip.is_loopback() && !ip.is_unspecified() {
                        addresses.push(ip);
                    }
                }
            }
            address = current.Next;
        }
        addresses
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use etherparse::{SlicedPacket, TransportSlice};
        use std::net::UdpSocket;

        #[test]
        #[ignore = "Requires Windows administrator permission; captures only synthetic local UDP"]
        fn native_compatibility_receives_udp_and_releases_sockets() -> Result<()> {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async {
                let address = active_addresses()?
                    .into_iter()
                    .next()
                    .context("No IPv4 adapter")?;
                let listener = UdpSocket::bind(SocketAddrV4::new(address, 22101))?;
                let sender = UdpSocket::bind(SocketAddrV4::new(address, 0))?;
                listener.set_read_timeout(Some(Duration::from_secs(2)))?;
                for _ in 0..2 {
                    let mut capture = CompatibilityCapture::open()?;
                    let marker = b"Irminsul synthetic capture verification";
                    sender.send_to(marker, listener.local_addr()?)?;
                    let mut received = [0u8; 128];
                    assert_eq!(listener.recv(&mut received)?, marker.len());
                    tokio::time::timeout(Duration::from_secs(5), async {
                        loop {
                            let frame = capture.next_packet().await?;
                            let packet = SlicedPacket::from_ethernet(&frame)?;
                            if let Some(TransportSlice::Udp(udp)) = packet.transport {
                                if udp.payload() == marker {
                                    return Ok::<_, anyhow::Error>(());
                                }
                            }
                        }
                    })
                    .await??;
                    drop(capture);
                }
                Ok(())
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use etherparse::PacketBuilder;

    fn datagram(source: u16, destination: u16) -> Vec<u8> {
        let builder =
            PacketBuilder::ipv4([192, 0, 2, 1], [192, 0, 2, 2], 64).udp(source, destination);
        let mut packet = Vec::new();
        builder.write(&mut packet, &[0, 1, 2, 3, 4, 5]).unwrap();
        packet
    }

    #[test]
    fn preserves_incoming_and_outgoing_game_datagrams() {
        for (src, dst) in [
            (50000, 22101),
            (22101, 50000),
            (50000, 22102),
            (22102, 50000),
        ] {
            let ip = datagram(src, dst);
            let frame = ethernet_frame(&ip).unwrap();
            assert_eq!(&frame[14..], ip);
            let packet = SlicedPacket::from_ethernet(&frame).unwrap();
            let Some(TransportSlice::Udp(udp)) = packet.transport else {
                panic!("UDP")
            };
            assert_eq!((udp.source_port(), udp.destination_port()), (src, dst));
            assert_eq!(udp.payload(), &[0, 1, 2, 3, 4, 5]);
        }
    }

    #[test]
    fn rejects_unrelated_malformed_and_truncated_packets() {
        assert!(ethernet_frame(&datagram(50000, 443)).is_none());
        assert!(ethernet_frame(&[0; 100]).is_none());
        let ip = datagram(50000, 22101);
        for length in 0..ip.len() {
            assert!(ethernet_frame(&ip[..length]).is_none());
        }
    }

    #[test]
    fn ignores_trailing_bytes_outside_ip_datagram() {
        let ip = datagram(22101, 50000);
        let mut padded = ip.clone();
        padded.extend_from_slice(&[0; 40]);
        assert_eq!(&ethernet_frame(&padded).unwrap()[14..], ip);
    }

    #[test]
    fn preserves_largest_ipv4_udp_datagram() {
        let builder = PacketBuilder::ipv4([192, 0, 2, 1], [192, 0, 2, 2], 64).udp(50000, 22101);
        let mut packet = Vec::new();
        builder.write(&mut packet, &vec![7; 65_507]).unwrap();
        assert_eq!(ethernet_frame(&packet).unwrap().len(), 65_549);
    }

    #[test]
    #[ignore = "Requires private recording manifest in IRMINSUL_REPLAY_MANIFEST"]
    fn private_recordings_match_after_ip_normalization() -> anyhow::Result<()> {
        use crate::Engine;
        use anyhow::{Context, ensure};
        use pcap_file::pcapng::{PcapNgReader, blocks::Block};
        use serde::Deserialize;
        use std::fs::File;
        #[derive(Deserialize)]
        struct Fixture {
            process: String,
            uid: String,
            path: String,
        }
        let fixtures: Vec<Fixture> =
            serde_json::from_reader(File::open(std::env::var("IRMINSUL_REPLAY_MANIFEST")?)?)?;
        ensure!(
            fixtures.len() >= 3,
            "Include account switching and restart fixtures"
        );
        let (mut original, mut compatibility) = (Engine::new()?, Engine::new()?);
        original.start()?;
        compatibility.start()?;
        for fixture in fixtures {
            original.observe_process(Some(fixture.process.clone()))?;
            compatibility.observe_process(Some(fixture.process))?;
            let mut reader = PcapNgReader::new(File::open(fixture.path)?)?;
            let mut forwarded = 0;
            while let Some(block) = reader.next_block() {
                if let Block::EnhancedPacket(packet) = block? {
                    original.receive(packet.data.to_vec())?;
                    if packet.data.get(12..14) == Some(&[8, 0]) {
                        if let Some(frame) = ethernet_frame(&packet.data[14..]) {
                            compatibility.receive(frame)?;
                            forwarded += 1;
                        }
                    }
                }
            }
            ensure!(forwarded > 0, "No IPv4 game packets in fixture");
            let expected = original.state();
            let actual = compatibility.state();
            ensure!(
                expected.phase == "ready",
                "Baseline fixture did not decode: {}",
                expected.message
            );
            ensure!(
                actual.phase == "ready",
                "Compatibility fixture did not decode: {}",
                actual.message
            );
            assert_eq!(actual.active_uid.as_deref(), Some(fixture.uid.as_str()));
            assert_eq!(actual.snapshots.len(), expected.snapshots.len());
            for reference in expected.snapshots {
                let found = actual
                    .snapshots
                    .iter()
                    .find(|s| s.uid == reference.uid)
                    .context("Missing account after switch")?;
                let expected_snapshot = original.snapshot(&reference.uid, &reference.capture_id)?;
                let actual_snapshot = compatibility.snapshot(&found.uid, &found.capture_id)?;
                for category in ["artifacts", "weapons", "characters", "materials"] {
                    assert_eq!(
                        actual_snapshot.good[category], expected_snapshot.good[category],
                        "{category}"
                    );
                }
                assert_eq!(
                    actual_snapshot.artifact_guids,
                    expected_snapshot.artifact_guids
                );
            }
        }
        Ok(())
    }
}
