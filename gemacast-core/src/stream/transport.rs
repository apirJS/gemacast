use std::io::Read;
use std::net::{SocketAddr, TcpStream, UdpSocket};

pub trait AudioTransport: Send {
    fn receive_packet(&mut self, buffer: &mut [u8]) -> std::io::Result<(usize, SocketAddr)>;
}

pub struct UdpTransport {
    pub socket: UdpSocket,
}

impl AudioTransport for UdpTransport {
    fn receive_packet(&mut self, buffer: &mut [u8]) -> std::io::Result<(usize, SocketAddr)> {
        self.socket.recv_from(buffer)
    }
}

pub struct TcpTransport {
    pub stream: TcpStream,
}

impl AudioTransport for TcpTransport {
    fn receive_packet(&mut self, buffer: &mut [u8]) -> std::io::Result<(usize, SocketAddr)> {
        let mut len_buf = [0u8; 4];

        self.stream.read_exact(&mut len_buf)?;

        let length = u32::from_be_bytes(len_buf) as usize;
        if length > buffer.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Packet too large for buffer",
            ));
        }

        self.stream.read_exact(&mut buffer[0..length])?;

        let addr = self
            .stream
            .peer_addr()
            .unwrap_or_else(|_| "127.0.0.1:55557".parse().unwrap());

        Ok((length, addr))
    }
}
