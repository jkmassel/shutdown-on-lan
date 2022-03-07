extern crate exitcode;

#[cfg(target_os = "macos")]
extern crate plist;

use std::path::PathBuf;
use std::path::Path;
use std::net::{IpAddr, Ipv4Addr};
use std::net::{SocketAddr, ToSocketAddrs};
use std::vec;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(not(windows))]
use std::fs::{ File };

#[cfg(not(windows))]
use std::io::{prelude::*, BufReader, BufWriter, Read};

#[cfg(windows)]
use winreg::RegKey;

#[cfg(windows)]
use winreg::enums::RegDisposition;

#[derive(Error, Debug)]
pub enum ConfigurationError {
    #[error("Failed to read app configuration")]
    UnableToReadConfiguration(IOErrorDetails),

    #[error("The configuration file on disk is corrupt")]
    InvalidConfigurationFile(ConfigurationFileDecodingErrorDetails),

    #[error("The configuration file in memory can't be converted to an on-disk representation")]
    InvalidConfiguration,

    #[cfg(windows)]
    #[error("Unable to write to a registry key")]
    RegistryKeyNotWritable(ConfigurationRegistryKeys),

    #[error("Unable to write to configuration storage directory")]
    ConfigurationStorageUnwritable(IOErrorDetails),

    #[error("Unable to write to configuration file: {}", exitcode::CANTCREAT)]
    ConfigurationFileUnwritable,

    #[error("Unable to write configuration – it is not valid")]
    ConfigurationSerializationError,
}

pub struct IOErrorDetails {
    path: String,
    code: i32,
    kind: std::io::ErrorKind,
}

impl std::fmt::Debug for IOErrorDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IOErrorDetails")
         .field("path", &self.path)
         .field("code", &self.code)
         .field("kind", &self.kind)
         .finish()
    }
}

impl IOErrorDetails {
    fn from_path(path: &Path, error: std::io::Error) -> Self{
        IOErrorDetails {
            path: path.to_str().unwrap().to_string(),
            code: exitcode::IOERR,
            kind: error.kind()
        }
    }
}

pub struct ConfigurationFileDecodingErrorDetails {
    path: String,
    message: String,
}

impl std::fmt::Debug for ConfigurationFileDecodingErrorDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigurationFileDecodingErrorDetails")
         .field("path", &self.path)
         .field("message", &self.message)
         .finish()
    }
}

#[cfg(target_os = "macos")]
impl ConfigurationFileDecodingErrorDetails {
     fn from_path(path: &Path, error: plist::Error) -> Self{
        Self {
            path: path.to_str().unwrap().to_string().clone(),
            message: format!("{:#?}", error),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AppConfiguration {
    pub port_number: u16,
    pub addresses: Vec<IpAddr>,
    pub secret: String,
}

impl AppConfiguration {

    pub fn validate() -> Result<(), ConfigurationError> {
        create_configuration_storage_if_not_exists()?;
        create_configuration_if_not_exists()?;
        AppConfiguration::fetch()?;

        Ok(())
    }

    fn default() -> AppConfiguration {
        AppConfiguration {
            port_number: 53632,
            addresses: [IpAddr::from(Ipv4Addr::new(127, 0, 0, 1))].to_vec(),
            secret: "Super Secret String".to_string(),
        }
    }

    #[cfg(target_os = "macos")]
    pub fn fetch() -> Result<AppConfiguration, ConfigurationError> {
        Plist::read_configuration(&configuration_file_path())
    }

    #[cfg(target_os = "linux")]
    pub fn fetch() -> Result<AppConfiguration, ConfigurationError> {
        extern crate serde_ini;

        let string = fs::read_to_string(configuration_file_path()).expect("Failed to read app configuration");
        let config: AppConfiguration = serde_ini::from_str(&string).expect("Invalid app configuration");
        config
    }

    #[cfg(windows)]
    pub fn fetch() -> Result<AppConfiguration, ConfigurationError> {
        log::info!("Looking up configuration");

        let registry = Registry::with_default_root_key()?;
        let ips_string = registry.read_string(ConfigurationRegistryKeys::IpAddress)?;
        let ips_list: Vec<&str> = ips_string.split(",").collect();

        let ip_addresses: Vec<IpAddr> = ips_list
            .iter()
            .map(|&ip| ip.parse().ok())
            .flat_map(|x| x)
            .collect();

        Ok(AppConfiguration {
            port_number: registry.read_u32(ConfigurationRegistryKeys::Port)? as u16,
            addresses: ip_addresses,
            secret: registry.read_string(ConfigurationRegistryKeys::Secret)?
        })
    }

    pub fn set_addresses(&mut self, string: String) {
        let ips_list: Vec<&str> = string.split(',').collect();

        self.addresses = ips_list
            .iter()
            .filter_map(|&ip| ip.parse().ok())
            .collect();
    }

    #[cfg(not(windows))]
    pub fn save(self) -> Result<(), ConfigurationError> {
        self.write(&configuration_file_path())?;
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn write(self, path: &Path) -> Result<(), ConfigurationError> {
        Plist::write_configuration(self, path.to_path_buf())
    }

    #[cfg(target_os = "linux")]
    fn write(self, path: &Path) {
        extern crate serde_ini;

        let string = serde_ini::to_string(&self).expect("Unable to serialize configuration");
        fs::write(path, string).expect("Unable to write configuration");
    }

    #[cfg(windows)]
    fn write(self) -> Result<(), ConfigurationError> {
        let registry = Registry::with_default_root_key()?;

        let addresses: Vec<String> = self
            .addresses
            .iter()
            .map(|ip| ip.to_string())
            .collect();

        let joined_addresses = addresses.join(",");
        registry.write_string(ConfigurationRegistryKeys::IpAddress, joined_addresses)?;
        log::debug!("Set IP Addresses to {}", joined_addresses);

        let u32_port_number = self.port_number as u32;
        registry.write_u32(ConfigurationRegistryKeys::Port, u32_port_number)?;
        log::debug!("Set Port to {}", u32_port_number);

        registry.write_string(ConfigurationRegistryKeys::Secret, self.secret)?;
        log::debug!("Set secret to {}", self.secret);

        Ok(())
    }
}

#[cfg(windows)]
#[derive(Debug)]
enum ConfigurationRegistryKeys {
    IpAddress,
    Port,
    Secret
}

#[cfg(windows)]
impl ConfigurationRegistryKeys {
    fn as_str(&self) -> &'static str {
        match self {
            ConfigurationRegistryKeys::IpAddress => "ip_addresses",
            ConfigurationRegistryKeys::Port => "port",
            ConfigurationRegistryKeys::Secret => "secret"
        }
    }
}

#[cfg(windows)]
impl AsRef<std::ffi::OsStr> for ConfigurationRegistryKeys {
    fn as_ref(&self) -> &std::ffi::OsStr {
        std::ffi::OsStr::new(self.as_str())
    }
}

#[cfg(windows)]
struct Registry{
    root_key: RegKey
}

#[cfg(windows)]
impl Registry {

    fn with_default_root_key() -> Result<(Registry), ConfigurationError> {
        Registry::with_root_key(Path::new("Software").join("ShutdownOnLan"))
    }

    fn with_root_key(path: PathBuf) -> Result<(Registry), ConfigurationError> {
        use winreg::enums::HKEY_CURRENT_USER;
        let (key, disposition) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(&path).expect("Failed to read app configuration");

        match disposition {
            RegDisposition::REG_CREATED_NEW_KEY => {
                log::debug!("Created New Registry Key");
            }
            RegDisposition::REG_OPENED_EXISTING_KEY => {
                log::debug!("Using Existing Registry Key");
            }
        }

        Ok(Registry {
            root_key: key,
        })
    }

    fn read(self) {

    }

    fn read_string(self, key: ConfigurationRegistryKeys) -> Result<String, ConfigurationError> {
        Ok("")
    }

    fn read_u32(self, key: ConfigurationRegistryKeys) -> Result<u32, ConfigurationError> {
        Ok(42)
    }

    fn write_string(self, key: ConfigurationRegistryKeys, value: String) -> Result<(), ConfigurationError> {
        match self.root_key.set_value(key, &value) {
            Ok(()) => Ok(()),
            Err(error) => Err(ConfigurationError::RegistryKeyNotWritable(key))
        }
    }

    fn write_u32(self,  key: ConfigurationRegistryKeys, value: u32) -> Result<(), ConfigurationError> {
        match self.root_key.set_value(key, &value) {
            Ok(()) => Ok(()),
            Err(error) => Err(ConfigurationError::RegistryKeyNotWritable(key))
        }
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
struct Plist{}

#[cfg(target_os = "macos")]
impl Plist {
    pub fn read_configuration(path: &Path) -> Result<AppConfiguration, ConfigurationError> {
        let file = File::open(path).map_err(|error|{
            ConfigurationError::UnableToReadConfiguration(IOErrorDetails::from_path(&path, error))
        })?;

        let mut reader = BufReader::new(file);
        let mut bytes = Vec::new();

        // Read the file into `bytes`
        reader.read_to_end(&mut bytes).map_err(|error|{
            ConfigurationError::UnableToReadConfiguration(IOErrorDetails::from_path(&path, error))
        })?;

        // Turn the bytes into a `plist`
        plist::from_bytes(&bytes).map_err(|error|{
            ConfigurationError::InvalidConfigurationFile(ConfigurationFileDecodingErrorDetails::from_path(&path, error))
        })
    }

    pub fn write_configuration(configuration: AppConfiguration, path: PathBuf) -> Result<(), ConfigurationError> {
        let mut file_handle = File::options().write(true).open(path).map_err(|_e|{ ConfigurationError::ConfigurationFileUnwritable })?;

        let mut buf = BufWriter::new(Vec::new());
        plist::to_writer_xml(&mut buf, &configuration).map_err(|_e|{ ConfigurationError::InvalidConfiguration })?;

        let bytes = buf.into_inner().unwrap();
        file_handle.write_all(&bytes).map_err(|_e|{ ConfigurationError::InvalidConfiguration })?;

        Ok(())
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

#[cfg(target_os = "linux")]
fn configuration_file_path() -> PathBuf {
    configuration_storage_path().join("ShutDownOnLan")
}

#[cfg(not(windows))]
fn create_configuration_storage_if_not_exists() -> Result<(), ConfigurationError> {
    let path = configuration_storage_path();
    if !path.exists() {
        std::fs::create_dir(&path).map_err(|error|{
            ConfigurationError::UnableToReadConfiguration(IOErrorDetails::from_path(&path, error))
        })?;
    }

    Ok(())
}

#[cfg(not(windows))]
fn create_configuration_if_not_exists() -> Result<(), ConfigurationError> {
    let configuration = AppConfiguration::default();
    configuration.write(&configuration_file_path())?;
    Ok(())
}

#[cfg(windows)]
fn create_configuration_storage_if_not_exists() -> Result<(), ConfigurationError> {
    Registry::with_default_root_key()?;
    Ok(())
}

#[cfg(windows)]
fn create_configuration_if_not_exists() -> Result<(), ConfigurationError> {
    let configuration = AppConfiguration::default();
    configuration.write();

    Ok(())
}

#[cfg(target_os = "macos")]
mod tests{
    #[test]
    fn test_that_configuration_path_is_valid() {
        
    }
}
