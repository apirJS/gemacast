use crate::domain::error::NetworkError;
use socket2::Socket;
use std::net::SocketAddr;

/// Owns the OS priority request for one outgoing audio socket.
pub(crate) struct AudioPriority {
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
}

impl AudioPriority {
    #[cfg(not(windows))]
    pub(crate) fn attach(socket: &Socket, _target: SocketAddr) -> Result<Self, NetworkError> {
        socket
            .set_tos_v4(0xB8)
            .map_err(|source| NetworkError::AudioPriorityFailed {
                operation: "IP_TOS EF",
                source,
            })?;
        Ok(Self {})
    }

    #[cfg(windows)]
    pub(crate) fn attach(socket: &Socket, target: SocketAddr) -> Result<Self, NetworkError> {
        use std::os::windows::io::AsRawSocket;
        use windows::Win32::{
            Foundation::HANDLE,
            NetworkManagement::QoS::{
                QOS_NON_ADAPTIVE_FLOW, QOS_VERSION, QOSAddSocketToFlow, QOSCreateHandle,
                QOSTrafficTypeVoice,
            },
            Networking::WinSock::SOCKET,
        };
        let mut handle = HANDLE::default();
        let version = QOS_VERSION {
            MajorVersion: 1,
            MinorVersion: 0,
        };
        // Pointers refer to live stack values for the duration of each synchronous call.
        if !unsafe { QOSCreateHandle(&version, &mut handle) }.as_bool() {
            return Err(NetworkError::AudioPriorityFailed {
                operation: "QOSCreateHandle",
                source: std::io::Error::last_os_error(),
            });
        }
        let priority = Self { handle };
        let destination = socket2::SockAddr::from(target);
        let mut flow = 0;
        if !unsafe {
            QOSAddSocketToFlow(
                handle,
                SOCKET(socket.as_raw_socket() as usize),
                Some(destination.as_ptr().cast()),
                QOSTrafficTypeVoice,
                QOS_NON_ADAPTIVE_FLOW,
                &mut flow,
            )
        }
        .as_bool()
        {
            return Err(NetworkError::AudioPriorityFailed {
                operation: "QOSAddSocketToFlow",
                source: std::io::Error::last_os_error(),
            });
        }
        Ok(priority)
    }
}

#[cfg(windows)]
impl Drop for AudioPriority {
    fn drop(&mut self) {
        // Closing our qWAVE handle removes its flows; it does not close the audio socket.
        unsafe { windows::Win32::NetworkManagement::QoS::QOSCloseHandle(self.handle) };
    }
}
