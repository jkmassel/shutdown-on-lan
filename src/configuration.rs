use std::net::{IpAddr, Ipv4Addr};
use std::net::{SocketAddr, ToSocketAddrs};
use std::vec;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct AppConfiguration {
    pub port_number: u16,
    pub addresses: Vec<IpAddr>,
    pub secret: String,
}

impl AppConfiguration {
    pub fn default() -> AppConfiguration {
        AppConfiguration {
            port_number: 53632,
            addresses: [IpAddr::from(Ipv4Addr::new(127, 0, 0, 1))].to_vec(),
            secret: "Super Secret String".to_string(),
        }
    }

    #[cfg(target_os = "macos")]
    pub fn fetch() -> AppConfiguration {
        extern crate plist;
        extern crate dirs;

        use std::fs;
        use std::path::Path;

        let mut path = dirs::home_dir()
            .expect("failed to find home directory")
            .join("Library")
            .join("Application Support")
            .join("ShutdownOnLan")
            .as_path()
            .to_owned();

        // If we're running as root, we can use the /Library path
        if path.to_str().unwrap().contains(&"root") {
            path = Path::new("/Library/Application Support/ShutdownOnLan").to_path_buf();
        }

        println!("path: {:?}", path);
        log::info!("path: {:?}", path);

        if !path.exists() {
            fs::create_dir(&path).expect("failed to create application configuration directory");
        }

        let file = path.join("ShutDownOnLan.plist");
        if !file.exists() {
            let default = AppConfiguration::default();
            plist::to_file_xml(file.clone(), &default)
                .expect("failed to write default app configuration");
            log::debug!("Created default configuration");
        }

        let config: AppConfiguration =
            plist::from_file(file).expect("failed to read app configuration");

        config
    }

    #[cfg(target_os = "linux")]
    pub fn fetch() -> AppConfiguration {
        AppConfiguration::default()
    }
}

impl ToSocketAddrs for AppConfiguration {
    type Iter = vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> std::io::Result<vec::IntoIter<SocketAddr>> {
        let mut addresses: Vec<SocketAddr> = Vec::new();

        log::info!("Read configuration with addresses: {:?}", self.addresses);

        for ip in self.addresses.clone() {
            // let mut address = SocketAddr::from((ip, self.port_number));
            addresses.push(SocketAddr::from((ip, self.port_number)));
        }

        log::info!("Translated configuration into: {:?}", addresses);

        let ret = addresses.into_iter();
        Ok(ret)
    }
}
