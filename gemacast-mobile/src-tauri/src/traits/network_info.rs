use crate::traits::types::InterfaceInfo;
use std::net::IpAddr;

/// Provides network interface and IP information.
///
/// **Production**: [`crate::adapters::NativeNetworkInfoProvider`]
/// **Tests**: [`crate::testing::mocks::MockNetworkInfoProvider`]
pub trait NetworkInfoProvider: Send + Sync {
    /// Get the local IP address.
    fn get_local_ip(&self) -> Result<IpAddr, String>;

    /// Get the default network interface info.
    fn get_default_interface(&self) -> Result<InterfaceInfo, String>;

    /// Get all network interfaces.
    fn get_interfaces(&self) -> Vec<InterfaceInfo>;
}
