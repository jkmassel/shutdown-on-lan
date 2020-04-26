use std::vec;
use std::net::{ IpAddr, Ipv4Addr };
use std::net::{ToSocketAddrs, SocketAddr};

pub struct AppConfiguration {
    pub port_number: u16,
    pub addresses: Vec<IpAddr>,
    pub secret: String,
}

impl AppConfiguration {
    pub fn default() -> AppConfiguration {
        AppConfiguration {
            port_number: 53632,
            addresses: [
                IpAddr::from(Ipv4Addr::new(127, 0, 0, 1)),
            ].to_vec(),
            secret: "Super Secret String".to_string(),
        }
    }
}

impl ToSocketAddrs for AppConfiguration {
    type Iter = vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> std::io::Result<vec::IntoIter<SocketAddr>> {
        let mut addresses : Vec<SocketAddr> = Vec::new();
        
        log::info!("Read configuration with addresses: {:?}", self.addresses);

        for ip in self.addresses.clone() {
            // let mut address = SocketAddr::from((ip, self.port_number));
            addresses.push(SocketAddr::from((ip, self.port_number)));
        }

        log::info!("Translated configuration into: {:?}", addresses);

        let ret = addresses.into_iter();
        return Ok(ret);
    }
}
