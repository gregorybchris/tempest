//! Asking the router for a mapping with UPnP-IGD, through `igd-next`.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use igd_next::aio::Gateway;
use igd_next::aio::tokio::{Tokio, search_gateway};
use igd_next::{
    AddAnyPortError, AddPortError, GatewayIpVersion, PortMappingProtocol, SearchOptions,
};

use super::{Granted, MappingError, local_ip_towards};

/// How long to wait for routers to answer the search.
const SEARCH_TIMEOUT: Duration = Duration::from_secs(3);
/// The description routers show for the mapping.
const DESCRIPTION: &str = "Tempest";

/// A client for one router's UPnP-IGD service.
#[derive(Debug)]
pub struct Client {
    gateway: Gateway<Tokio>,
    /// This peer's address on the router's network.
    local_ip: Ipv4Addr,
}

impl Client {
    /// Searches the network that `bind` is on (any, if unspecified) for a router.
    pub async fn find(bind: Ipv4Addr) -> Result<Self, MappingError> {
        let mut options = SearchOptions::default();
        options.bind_addr = SocketAddr::from((bind, 0));
        options.timeout = Some(SEARCH_TIMEOUT);
        options.single_search_timeout = Some(SEARCH_TIMEOUT);
        options.gateway_ip_version = GatewayIpVersion::V4;
        let gateway = search_gateway(options)
            .await
            .map_err(|_| MappingError::NoAnswer)?;
        let local_ip = match bind.is_unspecified() {
            true => local_ip_towards(gateway.addr.ip())?,
            false => bind,
        };
        Ok(Self { gateway, local_ip })
    }

    /// Asks for `internal_port` to be mapped for `lifetime`, at `suggested` if given and
    /// otherwise at the same port number if free.
    pub async fn map(
        &self,
        internal_port: u16,
        suggested: Option<SocketAddrV4>,
        lifetime: Duration,
    ) -> Result<Granted, MappingError> {
        let IpAddr::V4(ip) = self.gateway.get_external_ip().await.map_err(refused)? else {
            return Err(MappingError::Invalid("an IPv6 external address"));
        };
        let local = SocketAddr::from((self.local_ip, internal_port));
        let wanted = suggested.map_or(internal_port, |addr| addr.port());
        let secs = u32::try_from(lifetime.as_secs()).unwrap_or(u32::MAX);
        let (port, lifetime) = match self.add(wanted, local, secs).await {
            Ok(lifetime) => (wanted, lifetime),
            Err(AddPortError::PortInUse) => self.add_any(local, secs).await?,
            Err(error) => return Err(refused(error)),
        };
        Ok(Granted {
            external: SocketAddrV4::new(ip, port),
            lifetime,
        })
    }

    /// Maps `port` to `local`. Routers that only keep mappings until removed get one of those.
    async fn add(
        &self,
        port: u16,
        local: SocketAddr,
        secs: u32,
    ) -> Result<Option<Duration>, AddPortError> {
        let protocol = PortMappingProtocol::UDP;
        match self
            .gateway
            .add_port(protocol, port, local, secs, DESCRIPTION)
            .await
        {
            Ok(()) => Ok(Some(Duration::from_secs(secs.into()))),
            Err(AddPortError::OnlyPermanentLeasesSupported) => {
                let permanent = self.gateway.add_port(protocol, port, local, 0, DESCRIPTION);
                permanent.await.map(|()| None)
            }
            Err(error) => Err(error),
        }
    }

    /// Maps any free external port to `local`.
    async fn add_any(
        &self,
        local: SocketAddr,
        secs: u32,
    ) -> Result<(u16, Option<Duration>), MappingError> {
        let protocol = PortMappingProtocol::UDP;
        let lease = Some(Duration::from_secs(secs.into()));
        match self
            .gateway
            .add_any_port(protocol, local, secs, DESCRIPTION)
            .await
        {
            Ok(port) => Ok((port, lease)),
            Err(AddAnyPortError::OnlyPermanentLeasesSupported) => {
                let permanent = self.gateway.add_any_port(protocol, local, 0, DESCRIPTION);
                Ok((permanent.await.map_err(refused)?, None))
            }
            Err(error) => Err(refused(error)),
        }
    }

    /// Removes the mapping of `external_port`.
    pub async fn remove(&self, external_port: u16) -> Result<(), MappingError> {
        self.gateway
            .remove_port(PortMappingProtocol::UDP, external_port)
            .await
            .map_err(refused)
    }
}

fn refused(error: impl std::error::Error) -> MappingError {
    MappingError::Refused(error.to_string())
}
