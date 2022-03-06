use std::path::PathBuf;
use std::path::Path;
use std::net::{IpAddr, Ipv4Addr};
use std::net::{SocketAddr, ToSocketAddrs};
use std::vec;
use serde::{Deserialize, Serialize};

#[cfg(not(windows))]
use std::fs;

#[cfg(windows)]
use winreg::RegKey;

#[cfg(windows)]
use winreg::enums::RegDisposition;

#[derive(Serialize, Deserialize, Debug)]
pub struct AppConfiguration {
    pub port_number: u16,
    pub addresses: Vec<IpAddr>,
    pub secret: String,
}

impl AppConfiguration {

    pub fn validate() {
        create_configuration_storage_if_not_exists();
        create_configuration_if_not_exists();
        AppConfiguration::fetch();
    }

    fn default() -> AppConfiguration {
        AppConfiguration {
            port_number: 53632,
            addresses: [IpAddr::from(Ipv4Addr::new(127, 0, 0, 1))].to_vec(),
            secret: "Super Secret String".to_string(),
        }
    }

    #[cfg(target_os = "macos")]
    pub fn fetch() -> AppConfiguration {
        extern crate plist;

        let config: AppConfiguration = plist::from_file(configuration_file_path()).expect("Failed to read app configuration");
        config
    }

    #[cfg(target_os = "linux")]
    pub fn fetch() -> AppConfiguration {
        extern crate serde_ini;

        let string = fs::read_to_string(configuration_file_path()).expect("Failed to read app configuration");
        let config: AppConfiguration = serde_ini::from_str(&string).expect("Failed to read app configuration");
        config
    }

    #[cfg(windows)]
    pub fn fetch() -> AppConfiguration {
        use winreg::enums::REG_OPENED_EXISTING_KEY;

        const IP_ADDRESS_KEY: &str = "ip_addresses";
        const PORT_KEY: &str = "port";
        const SECRET_KEY: &str = "secret";

        log::info!("Looking up configuration");

        let (key, disposition) = open_registry_key(configuration_storage_path());
        assert!(disposition == REG_OPENED_EXISTING_KEY);

        let ips_string: String = key.get_value(IP_ADDRESS_KEY).expect("Failed to read app configuration");
        let ips_list: Vec<&str> = ips_string.split(",").collect();

        let ip_addresses = ips_list
            .iter()
            .map(|&ip| ip.parse().ok())
            .flat_map(|x| x)
            .collect();

        let port: u32 = key.get_value(PORT_KEY).expect("Failed to read app configuration");
        let secret: String = key.get_value(SECRET_KEY).expect("Failed to read app configuration");

        return AppConfiguration {
            port_number: port as u16,
            addresses: ip_addresses,
            secret,
        }
    }

    #[cfg(target_os = "macos")]
    fn write(self, path: &Path) {
        extern crate plist;

        match plist::to_file_xml(path, &self) {
            Ok(file) => file,
            Err(error) => panic!("Problem writing the configuration: {:?}", error),
        };
    }

    #[cfg(target_os = "linux")]
    fn write(self, path: &Path) {
        extern crate serde_ini;

        let string = serde_ini::to_string(&self).expect("Unable to serialize configuration");
        fs::write(path, string).expect("Unable to write configuration");
    }

    #[cfg(windows)]
    fn write(self) {

        const IP_ADDRESS_KEY: &str = "ip_addresses";
        const PORT_KEY: &str = "port";
        const SECRET_KEY: &str = "secret";

        let (key, disposition) = open_registry_key(configuration_storage_path());
        assert!(disposition == RegDisposition::REG_OPENED_EXISTING_KEY);

        let addresses: Vec<String> = self
            .addresses
            .iter()
            .map(|ip| ip.to_string())
            .collect();

        let joined_addresses = addresses.join(",");
        key.set_value(IP_ADDRESS_KEY, &joined_addresses).expect("Unable to write configuration");
        log::debug!("Set IP Addresses to {}", joined_addresses);

        let u32_port_number = self.port_number as u32;
        key.set_value(PORT_KEY, &u32_port_number).expect("Unable to write configuration");
        log::debug!("Set Port to {}", u32_port_number);

        key.set_value(SECRET_KEY, &self.secret).expect("Unable to write configuration");
        log::debug!("Set secret to {}", self.secret);
    }
}

impl ToSocketAddrs for AppConfiguration {
    type Iter = vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> std::io::Result<vec::IntoIter<SocketAddr>> {
        let mut addresses: Vec<SocketAddr> = Vec::new();

        log::info!("Read configuration with addresses: {:?}", self.addresses);

        for ip in self.addresses.clone() {
            addresses.push(SocketAddr::from((ip, self.port_number)));
        }

        log::info!("Translated configuration into: {:?}", addresses);

        let ret = addresses.into_iter();
        Ok(ret)
    }
}

#[cfg(target_os = "macos")]
fn configuration_storage_path() -> PathBuf {
    extern crate dirs;

    let username = whoami::username();

    if username == "root" {
        let path = Path::new("/Library/Application Support/ShutdownOnLan").to_path_buf();
        log::info!("Detected Configuration Path: {:?}", path);
        return path
    }

    let path = dirs::home_dir()
        .expect("failed to find home directory")
        .join("Library")
        .join("Application Support")
        .join("ShutdownOnLan")
        .as_path()
        .to_owned();

    log::info!("Detected Configuration Path: {:?}", path);

    path
}

#[cfg(target_os = "linux")]
fn configuration_storage_path() -> PathBuf {
    Path::new("/usr/local/etc").to_path_buf()
}

#[cfg(target_os = "macos")]
fn configuration_file_path() -> PathBuf {
    configuration_storage_path().join("ShutDownOnLan.plist")
}

#[cfg(windows)]
fn configuration_storage_path() -> PathBuf {
    Path::new("Software").join("ShutdownOnLan")
}

#[cfg(target_os = "linux")]
fn configuration_file_path() -> PathBuf {
    configuration_storage_path().join("ShutDownOnLan")
}

#[cfg(not(windows))]
fn create_configuration_storage_if_not_exists() {
    let path = configuration_storage_path();
    if !path.exists() {
        fs::create_dir(&path).expect("Unable to create application configuration directory – exiting.");
    }
}

#[cfg(not(windows))]
fn create_configuration_if_not_exists() {
    let configuration = AppConfiguration::default();
    configuration.write(&configuration_file_path());
}

#[cfg(windows)]
fn create_configuration_storage_if_not_exists() {
    let (_key, disposition) = open_registry_key(configuration_storage_path());

    match disposition {
        RegDisposition::REG_CREATED_NEW_KEY => {
            log::debug!("Created New Registry Key");
        }
        RegDisposition::REG_OPENED_EXISTING_KEY => {
            log::debug!("Using Existing Registry Key");
        }
    }
}

#[cfg(windows)]
fn open_registry_key(key: PathBuf) -> (RegKey, RegDisposition) {
    use winreg::enums::HKEY_CURRENT_USER;
    RegKey::predef(HKEY_CURRENT_USER).create_subkey(key).expect("Failed to read app configuration")
}

#[cfg(windows)]
fn create_configuration_if_not_exists() {
    let configuration = AppConfiguration::default();
    configuration.write();
}
