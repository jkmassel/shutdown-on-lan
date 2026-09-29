use serde::{Deserialize, Serialize};
#[cfg(not(target_os = "linux"))]
use std::net::Ipv4Addr;
use std::net::{AddrParseError, IpAddr};
#[cfg(not(windows))]
use std::path::Path;
use std::path::PathBuf;
use thiserror::Error;

#[cfg(windows)]
use winreg::RegKey;

#[cfg(windows)]
use winreg::enums::RegDisposition;

#[cfg(target_os = "macos")]
use core_foundation::{
    array::CFArray,
    base::{CFType, TCFType},
    data::CFData,
    dictionary::CFDictionary,
    number::CFNumber,
    propertylist::{CFPropertyList, CFPropertyListSubClass},
    string::{CFString, CFStringRef},
};
#[cfg(target_os = "macos")]
use std::convert::TryFrom;

/// The longest secret we'll accept, in bytes.
pub const MAX_SECRET_LENGTH: usize = 4096;

/// The secret every installation shared before 0.4.0. It was published in the README, so anyone on the
/// network could use it to shut down a machine that still has it.
const LEGACY_DEFAULT_SECRET: &str = "Super Secret String";

pub const LEGACY_DEFAULT_SECRET_WARNING: &str = "The secret is still the default from versions before 0.4.0, which was published – anyone on the network can use it to shut this machine down. Change it with `shutdown-on-lan set --secret`, then update your control system.";

/// The interface address every installation defaulted to before 0.4.0 – see `forget_legacy_default_addresses`.
#[cfg(not(target_os = "linux"))]
const LEGACY_DEFAULT_ADDRESS: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

// Unknown keys are rejected, because a missing `allowed_sources` allows any client – a misspelled one would
// otherwise turn the allowlist off without any warning
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppConfiguration {
    pub port_number: u16,
    /// The local interface addresses to accept connections on. Empty means every interface.
    pub addresses: Vec<IpAddr>,
    pub secret: String,
    /// The client addresses allowed to connect. Empty means any client may connect.
    // Missing from configurations written by older versions, so default to allowing any client
    #[serde(default)]
    pub allowed_sources: Vec<IpAddr>,
}

impl AppConfiguration {
    /// Reads the configuration, creating it from defaults first if needed.
    pub fn load() -> Result<AppConfiguration, ConfigurationError> {
        Self::create_configuration_if_not_exists()?;
        Self::fetch()
    }

    /// Checks the values that can't be checked while parsing, because they could have been written by hand
    /// (or managed by a configuration profile) – for instance, an empty secret, which would never match.
    pub fn validate(&self) -> Result<(), ConfigurationError> {
        validate_port(self.port_number)?;
        validate_secret(&self.secret)?;
        validate_allowed_sources(&self.allowed_sources)
    }

    /// Whether the secret is the one every installation shared before 0.4.0. It's kept on upgrade, because
    /// replacing it would break every control system that uses it, but it should be changed.
    pub fn uses_legacy_default_secret(&self) -> bool {
        self.secret == LEGACY_DEFAULT_SECRET
    }

    /// Versions before 0.4.0 defaulted `addresses` to `127.0.0.1`, but never enforced it – they accepted
    /// connections on every interface. Enforcing it now would drop every connection from the network, so a
    /// configuration upgraded from one of them that still has the old default keeps accepting connections
    /// on every interface.
    ///
    /// On Linux, the configuration moved to a different file in 0.4.0, and the old one isn't imported.
    #[cfg(not(target_os = "linux"))]
    fn forget_legacy_default_addresses(&mut self) {
        if self.addresses == [LEGACY_DEFAULT_ADDRESS] {
            log::info!(
                "Accepting connections on every interface rather than only {}, which was the default before 0.4.0 but was never enforced",
                LEGACY_DEFAULT_ADDRESS
            );
            self.addresses.clear();
        }
    }

    /// Whether a connection received on the local interface `ip` is allowed to shut down the machine.
    pub fn accepts_connections_on(&self, ip: &IpAddr) -> bool {
        self.addresses.is_empty()
            || self
                .addresses
                .iter()
                .any(|address| interface_matches(address, ip))
    }

    /// Whether a client at `ip` is allowed to connect.
    pub fn accepts_connections_from(&self, ip: &IpAddr) -> bool {
        self.allowed_sources.is_empty() || contains_address(&self.allowed_sources, ip)
    }
}

/// Changes made with `set`. Only the values being changed are read and written, so `set` can replace a
/// stored value that's invalid – loading the whole configuration first would fail on it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ConfigurationUpdate {
    pub port_number: Option<u16>,
    pub addresses: Option<Vec<IpAddr>>,
    pub secret: Option<String>,
    pub allowed_sources: Option<Vec<IpAddr>>,
}

impl ConfigurationUpdate {
    pub fn set_port(&mut self, port: u16) -> Result<(), ConfigurationError> {
        validate_port(port)?;
        self.port_number = Some(port);
        Ok(())
    }

    /// Sets the local interface addresses from a comma-separated list. An empty string accepts
    /// connections on every interface.
    pub fn set_addresses(&mut self, string: &str) -> Result<(), ConfigurationError> {
        self.addresses = Some(parse_optional_addresses(string)?);
        Ok(())
    }

    pub fn set_secret(&mut self, secret: String) -> Result<(), ConfigurationError> {
        validate_secret(&secret)?;
        self.secret = Some(secret);
        Ok(())
    }

    /// Sets the allowed client addresses from a comma-separated list. An empty string allows any client.
    pub fn set_allowed_sources(&mut self, string: &str) -> Result<(), ConfigurationError> {
        let allowed_sources = parse_optional_addresses(string)?;
        validate_allowed_sources(&allowed_sources)?;
        self.allowed_sources = Some(allowed_sources);
        Ok(())
    }
}

/// Every value, for writing a whole configuration.
impl From<&AppConfiguration> for ConfigurationUpdate {
    fn from(configuration: &AppConfiguration) -> Self {
        ConfigurationUpdate {
            port_number: Some(configuration.port_number),
            addresses: Some(configuration.addresses.clone()),
            secret: Some(configuration.secret.clone()),
            allowed_sources: Some(configuration.allowed_sources.clone()),
        }
    }
}

/// Whether a connection received on the interface `ip` matches the configured `address`. As when binding a
/// socket, `0.0.0.0` means every IPv4 interface and `::` every interface. Versions before 0.4.0 accepted
/// them too, and accepted connections on every interface whatever the configuration said.
fn interface_matches(address: &IpAddr, ip: &IpAddr) -> bool {
    match address.to_canonical() {
        IpAddr::V4(address) if address.is_unspecified() => ip.to_canonical().is_ipv4(),
        IpAddr::V6(address) if address.is_unspecified() => true,
        address => address == ip.to_canonical(),
    }
}

/// Treats an IPv4 address and the IPv4-mapped IPv6 form of it (`::ffff:10.0.1.50`) as the same address.
fn contains_address(addresses: &[IpAddr], ip: &IpAddr) -> bool {
    addresses
        .iter()
        .any(|address| address.to_canonical() == ip.to_canonical())
}

/// Port 0 would listen on a different, random port every time the service starts.
fn validate_port(port: u16) -> Result<(), ConfigurationError> {
    if port == 0 {
        return Err(ConfigurationError::InvalidPort);
    }

    Ok(())
}

/// `0.0.0.0` and `::` aren't addresses a client can connect from, so they'd never match – leaving the list
/// empty is how to allow any client.
fn validate_allowed_sources(addresses: &[IpAddr]) -> Result<(), ConfigurationError> {
    match addresses.iter().find(|address| address.is_unspecified()) {
        Some(address) => Err(ConfigurationError::UnspecifiedSource(*address)),
        None => Ok(()),
    }
}

fn validate_secret(secret: &str) -> Result<(), ConfigurationError> {
    if secret.is_empty() || secret.len() > MAX_SECRET_LENGTH {
        return Err(ConfigurationError::InvalidSecret);
    }

    Ok(())
}

pub fn parse_addresses(string: &str) -> Result<Vec<IpAddr>, AddrParseError> {
    string.split(',').map(|ip| ip.trim().parse()).collect()
}

/// Like `parse_addresses`, but an empty string is an empty list rather than an error.
pub fn parse_optional_addresses(string: &str) -> Result<Vec<IpAddr>, AddrParseError> {
    if string.trim().is_empty() {
        return Ok(Vec::new());
    }

    parse_addresses(string)
}

/// Like `format_addresses`, but describes an empty list of interface addresses as meaning every interface.
pub fn describe_addresses(addresses: &[IpAddr]) -> String {
    if addresses.is_empty() {
        "every interface".to_string()
    } else {
        format_addresses(addresses)
    }
}

pub fn format_addresses(addresses: &[IpAddr]) -> String {
    addresses
        .iter()
        .map(|ip| ip.to_string())
        .collect::<Vec<String>>()
        .join(",")
}

#[cfg(target_os = "macos")]
impl AppConfiguration {
    pub fn fetch() -> Result<AppConfiguration, ConfigurationError> {
        log::debug!("Fetching App Configuration");
        Self::fetch_from(&Storage::system())
    }

    pub fn create_configuration_if_not_exists() -> Result<(), ConfigurationError> {
        log::debug!("Checking whether configuration needs to be created");
        Self::prepare(&Storage::system(), Path::new(LEGACY_CONFIGURATION_FILE))
    }

    /// Imports anything written by older versions, then fills in any missing values.
    fn prepare(storage: &Storage, legacy_file: &Path) -> Result<(), ConfigurationError> {
        // Before anything writes to the preferences domain, which would replace the file
        storage.check_preferences_file()?;

        if legacy_file.exists() {
            Self::migrate_legacy_file(legacy_file, storage)?;
        }

        Self::migrate_secret_from_preferences(storage)?;
        Self::write_missing_defaults(storage)
    }

    fn fetch_from(storage: &Storage) -> Result<AppConfiguration, ConfigurationError> {
        let secret = storage
            .read_secret()?
            .ok_or(ConfigurationError::PreferenceNotReadable(
                PreferenceKeys::SECRET,
            ))?;

        Self::from_property_lists(|key| storage.preferences.get(key), secret)
            .map_err(ConfigurationError::PreferenceNotReadable)
    }

    /// Writes defaults for any values that are missing. Existing values (including those managed by a
    /// configuration profile) are never overwritten.
    fn write_missing_defaults(storage: &Storage) -> Result<(), ConfigurationError> {
        let defaults = AppConfiguration::default();
        let preferences = &storage.preferences;
        let mut changed = false;

        for (key, value) in defaults.to_property_lists() {
            if preferences.get(key).is_none() {
                log::info!("Writing default {} to preferences", key);
                preferences.set(key, &value);
                changed = true;
            }
        }

        if changed {
            preferences.synchronize()?;
        }

        if storage.read_secret()?.is_none() {
            log::info!(
                "Writing a random secret to {}",
                storage.secret_file.display()
            );
            storage.write_secret(&defaults.secret)?;
        }

        Ok(())
    }

    /// Imports the plist written by versions before configuration moved to `CFPreferences`, then deletes
    /// it so it isn't imported again. Only the system-wide file is imported – files under users' home
    /// directories were written by running the CLI without `sudo`, and the service never read them.
    fn migrate_legacy_file(path: &Path, storage: &Storage) -> Result<(), ConfigurationError> {
        log::info!("Migrating configuration from {}", path.display());
        let preferences = &storage.preferences;

        let bytes = std::fs::read(path).map_err(ConfigurationError::MissingConfigurationFile)?;
        let dictionary =
            parse_dictionary(&bytes).ok_or(ConfigurationError::CorruptConfigurationFile)?;

        let get = |key: &str| {
            dictionary
                .find(CFString::new(key).as_CFTypeRef())
                .map(|value| unsafe { CFPropertyList::wrap_under_get_rule(*value) })
        };
        let secret = get(PreferenceKeys::SECRET)
            .and_then(|value| value.downcast_into::<CFString>())
            .ok_or(ConfigurationError::CorruptConfigurationFile)?
            .to_string();
        let mut legacy = Self::from_property_lists(get, secret)
            .map_err(|_key| ConfigurationError::CorruptConfigurationFile)?;
        legacy.forget_legacy_default_addresses();

        // Values managed by a configuration profile take precedence over the legacy file
        for (key, value) in legacy.to_property_lists() {
            if !preferences.is_forced(key) {
                preferences.set(key, &value);
            }
        }
        preferences.synchronize()?;

        if !preferences.is_forced(PreferenceKeys::SECRET) {
            storage.write_secret(&legacy.secret)?;
        }

        std::fs::remove_file(path).map_err(|error| {
            ConfigurationError::ConfigurationFileUnwritable {
                source: error,
                path: path.display().to_string(),
            }
        })?;

        log::info!("Migrated configuration to {}", PREFERENCES_FILE);
        Ok(())
    }

    /// Moves a secret stored in the preferences domain (for instance with `defaults write`) into the secret
    /// file, because any local user can read the preferences domain. A secret managed by a configuration
    /// profile is left alone – it can't be removed here.
    fn migrate_secret_from_preferences(storage: &Storage) -> Result<(), ConfigurationError> {
        let preferences = &storage.preferences;

        if preferences.is_forced(PreferenceKeys::SECRET) {
            return Ok(());
        }

        // Only look in the domain this writes to, because that's the only one it can remove the secret from
        let secret = match preferences.get_local(PreferenceKeys::SECRET) {
            Some(value) => value
                .downcast_into::<CFString>()
                .ok_or(ConfigurationError::PreferenceNotReadable(
                    PreferenceKeys::SECRET,
                ))?
                .to_string(),
            None => return Ok(()),
        };
        validate_secret(&secret)?;

        log::info!(
            "Moving the secret from the preferences domain to {}",
            storage.secret_file.display()
        );
        storage.write_secret(&secret)?;
        preferences.remove(PreferenceKeys::SECRET);
        preferences.synchronize()
    }

    /// Reads a configuration from property list values, returning the key of the first value that's
    /// missing or has the wrong type. `allowed_sources` may be missing, because older versions didn't
    /// write it.
    fn from_property_lists(
        get: impl Fn(&'static str) -> Option<CFPropertyList>,
        secret: String,
    ) -> Result<AppConfiguration, &'static str> {
        let allowed_sources = match get(PreferenceKeys::ALLOWED_SOURCES) {
            Some(value) => {
                property_list_to_addresses(value).ok_or(PreferenceKeys::ALLOWED_SOURCES)?
            }
            None => Vec::new(),
        };

        Ok(AppConfiguration {
            port_number: get(PreferenceKeys::PORT)
                .and_then(property_list_to_u16)
                .ok_or(PreferenceKeys::PORT)?,
            addresses: get(PreferenceKeys::ADDRESSES)
                .and_then(property_list_to_addresses)
                .ok_or(PreferenceKeys::ADDRESSES)?,
            secret,
            allowed_sources,
        })
    }

    /// Every value except the secret, which is stored separately.
    fn to_property_lists(&self) -> Vec<(&'static str, CFPropertyList)> {
        ConfigurationUpdate::from(self).to_property_lists()
    }
}

#[cfg(target_os = "macos")]
impl ConfigurationUpdate {
    pub fn apply(&self) -> Result<(), ConfigurationError> {
        AppConfiguration::create_configuration_if_not_exists()?;
        self.apply_to(&Storage::system())
    }

    /// Writes the values that differ from the stored ones. Fails without writing anything if one of them
    /// is managed by a configuration profile, because the change would have no effect.
    fn apply_to(&self, storage: &Storage) -> Result<(), ConfigurationError> {
        storage.check_preferences_file()?;
        let preferences = &storage.preferences;

        let changes: Vec<(&'static str, CFPropertyList)> = self
            .to_property_lists()
            .into_iter()
            .filter(|(key, value)| preferences.get(key).as_ref() != Some(value))
            .collect();
        // A stored secret that can't be read is replaced, rather than stopping it from being replaced
        let secret = self
            .secret
            .as_deref()
            .filter(|secret| storage.read_secret().ok().flatten().as_deref() != Some(*secret));

        let managed_key = changes
            .iter()
            .map(|(key, _)| *key)
            .chain(secret.map(|_| PreferenceKeys::SECRET))
            .find(|key| preferences.is_forced(key));
        if let Some(key) = managed_key {
            return Err(ConfigurationError::PreferenceIsManaged(key));
        }

        if let Some(secret) = secret {
            log::debug!("Setting secret");
            storage.write_secret(secret)?;
        }

        for (key, value) in &changes {
            log::debug!("Setting {}", key);
            preferences.set(key, value);
        }

        if changes.is_empty() {
            Ok(())
        } else {
            preferences.synchronize()
        }
    }

    /// The values being changed, except the secret, which is stored separately.
    fn to_property_lists(&self) -> Vec<(&'static str, CFPropertyList)> {
        [
            self.port_number.map(|port| {
                (
                    PreferenceKeys::PORT,
                    CFNumber::from(port as i32).into_CFPropertyList(),
                )
            }),
            self.addresses.as_ref().map(|addresses| {
                (
                    PreferenceKeys::ADDRESSES,
                    addresses_to_property_list(addresses),
                )
            }),
            self.allowed_sources.as_ref().map(|addresses| {
                (
                    PreferenceKeys::ALLOWED_SOURCES,
                    addresses_to_property_list(addresses),
                )
            }),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

#[cfg(target_os = "linux")]
impl AppConfiguration {
    pub fn fetch() -> Result<AppConfiguration, ConfigurationError> {
        Self::from_toml(&Self::read_configuration_file()?)
    }

    pub fn save(&self) -> Result<(), ConfigurationError> {
        Self::write_configuration_file(&self.to_toml()?)
    }

    fn read_configuration_file() -> Result<String, ConfigurationError> {
        std::fs::read_to_string(Self::configuration_file_path()).map_err(|error| {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                ConfigurationError::RequiresRoot
            } else {
                ConfigurationError::InvalidConfigurationFile { source: error }
            }
        })
    }

    fn write_configuration_file(string: &str) -> Result<(), ConfigurationError> {
        let path = PathBuf::from(Self::configuration_file_path());
        write_private_file(&path, string.as_bytes()).map_err(|error| {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                ConfigurationError::RequiresRoot
            } else {
                ConfigurationError::ConfigurationFileUnwritable {
                    source: error,
                    path: path.into_os_string().into_string().unwrap(),
                }
            }
        })
    }

    fn from_toml(string: &str) -> Result<AppConfiguration, ConfigurationError> {
        toml::from_str(string).map_err(ConfigurationError::CorruptTomlConfigurationFile)
    }

    fn to_toml(&self) -> Result<String, ConfigurationError> {
        toml::to_string(self).map_err(|_e| ConfigurationError::InvalidConfiguration)
    }

    fn configuration_storage_path() -> String {
        Path::new("/etc").to_str().unwrap().to_string()
    }

    fn configuration_file_path() -> String {
        PathBuf::from(Self::configuration_storage_path())
            .join("shutdown-on-lan.toml")
            .into_os_string()
            .to_str()
            .unwrap()
            .to_string()
    }

    fn create_configuration_storage_if_not_exists() -> Result<(), ConfigurationError> {
        let path = PathBuf::from(Self::configuration_storage_path());

        if path.exists() && path.is_dir() {
            return Ok(());
        }

        log::debug!("Creating configuration storage at {:?}", path);

        std::fs::create_dir(&path).map_err(|error| {
            ConfigurationError::ConfigurationStorageUnwritable {
                source: error,
                path: path.into_os_string().into_string().unwrap(),
            }
        })
    }

    fn create_configuration_if_not_exists() -> Result<(), ConfigurationError> {
        log::debug!("Checking whether configuration needs to be created");

        Self::create_configuration_storage_if_not_exists()?;

        let path = PathBuf::from(Self::configuration_file_path());

        if path.exists() {
            log::debug!("Configuration Exists");
            return Ok(());
        }

        log::info!("Creating Configuration File from Defaults");

        let configuration = AppConfiguration::default();

        log::debug!("Creating configuration at {:?}", path);

        configuration.save()
    }
}

#[cfg(target_os = "linux")]
impl ConfigurationUpdate {
    pub fn apply(&self) -> Result<(), ConfigurationError> {
        AppConfiguration::create_configuration_if_not_exists()?;
        let updated = self.apply_to_toml(&AppConfiguration::read_configuration_file()?)?;
        AppConfiguration::write_configuration_file(&updated)
    }

    /// Replaces the values being changed in a configuration file. The others are kept as they are, without
    /// being parsed, so one that's invalid doesn't stop the others from being changed.
    fn apply_to_toml(&self, string: &str) -> Result<String, ConfigurationError> {
        let mut table: toml::Table = string
            .parse()
            .map_err(ConfigurationError::CorruptTomlConfigurationFile)?;

        // The same names that `AppConfiguration`'s fields have
        let mut set = |key: &str, value: Option<toml::Value>| {
            if let Some(value) = value {
                table.insert(key.to_string(), value);
            }
        };
        let addresses = |addresses: &Vec<IpAddr>| {
            toml::Value::Array(
                addresses
                    .iter()
                    .map(|ip| toml::Value::String(ip.to_string()))
                    .collect(),
            )
        };
        set(
            "port_number",
            self.port_number
                .map(|port| toml::Value::Integer(port.into())),
        );
        set("addresses", self.addresses.as_ref().map(addresses));
        set("secret", self.secret.clone().map(toml::Value::String));
        set(
            "allowed_sources",
            self.allowed_sources.as_ref().map(addresses),
        );

        toml::to_string(&table).map_err(|_error| ConfigurationError::InvalidConfiguration)
    }
}

#[cfg(windows)]
impl AppConfiguration {
    pub fn fetch() -> Result<AppConfiguration, ConfigurationError> {
        log::info!("Looking up configuration");
        Self::fetch_from(&Registry::with_default_root_key()?)
    }

    pub fn create_configuration_if_not_exists() -> Result<(), ConfigurationError> {
        log::info!("Checking whether configuration needs to be created");
        let registry = Registry::with_default_root_key()?;
        registry.restrict_access()?;
        Self::write_missing_defaults(&registry)
    }

    fn fetch_from(registry: &Registry) -> Result<AppConfiguration, ConfigurationError> {
        let ips_string = registry.read_string(ConfigurationRegistryKeys::IpAddress)?;

        Ok(AppConfiguration {
            port_number: registry.read_u16(ConfigurationRegistryKeys::Port)?,
            addresses: parse_registry_addresses(&ips_string),
            secret: registry.read_string(ConfigurationRegistryKeys::Secret)?,
            allowed_sources: parse_optional_addresses(
                &registry.read_string(ConfigurationRegistryKeys::AllowedSources)?,
            )
            .map_err(|_error| {
                ConfigurationError::RegistryKeyNotReadable(
                    ConfigurationRegistryKeys::AllowedSources,
                )
            })?,
        })
    }

    /// Like `forget_legacy_default_addresses`, for the value stored in the registry.
    fn forget_legacy_default_addresses_in(registry: &Registry) -> Result<(), ConfigurationError> {
        let stored = registry.read_string(ConfigurationRegistryKeys::IpAddress)?;
        let mut configuration = AppConfiguration {
            addresses: parse_registry_addresses(&stored),
            ..AppConfiguration::default()
        };
        configuration.forget_legacy_default_addresses();

        if configuration.addresses.is_empty() && !stored.trim().is_empty() {
            registry.write_string(ConfigurationRegistryKeys::IpAddress, &String::new())?;
        }

        Ok(())
    }

    /// Writes defaults for any values that are missing from the registry. Existing values are never
    /// overwritten – if one of them exists but can't be read, that's reported as an error instead.
    fn write_missing_defaults(registry: &Registry) -> Result<(), ConfigurationError> {
        let defaults = AppConfiguration::default();

        if !registry.contains(ConfigurationRegistryKeys::IpAddress)? {
            log::info!("Writing default IP addresses to registry");
            registry.write_string(
                ConfigurationRegistryKeys::IpAddress,
                &format_addresses(&defaults.addresses),
            )?;
        }

        if !registry.contains(ConfigurationRegistryKeys::Port)? {
            log::info!("Writing default port to registry");
            registry.write_u32(ConfigurationRegistryKeys::Port, defaults.port_number as u32)?;
        }

        if !registry.contains(ConfigurationRegistryKeys::Secret)? {
            log::info!("Writing default secret to registry");
            registry.write_string(ConfigurationRegistryKeys::Secret, &defaults.secret)?;
        }

        // Only versions before 0.4.0 left `allowed_sources` out, so this is an upgrade from one of them
        if !registry.contains(ConfigurationRegistryKeys::AllowedSources)? {
            Self::forget_legacy_default_addresses_in(registry)?;

            log::info!("Writing default allowed sources to registry");
            registry.write_string(
                ConfigurationRegistryKeys::AllowedSources,
                &format_addresses(&defaults.allowed_sources),
            )?;
        }

        Ok(())
    }
}

#[cfg(windows)]
impl ConfigurationUpdate {
    pub fn apply(&self) -> Result<(), ConfigurationError> {
        AppConfiguration::create_configuration_if_not_exists()?;
        self.apply_to(&Registry::with_default_root_key()?)
    }

    fn apply_to(&self, registry: &Registry) -> Result<(), ConfigurationError> {
        if let Some(addresses) = &self.addresses {
            let joined_addresses = format_addresses(addresses);
            registry.write_string(ConfigurationRegistryKeys::IpAddress, &joined_addresses)?;
            log::debug!("Set IP Addresses to {joined_addresses}");
        }

        if let Some(port) = self.port_number {
            registry.write_u32(ConfigurationRegistryKeys::Port, port.into())?;
            log::debug!("Set Port to {port}");
        }

        if let Some(secret) = &self.secret {
            registry.write_string(ConfigurationRegistryKeys::Secret, secret)?;
            log::debug!("Set secret");
        }

        if let Some(allowed_sources) = &self.allowed_sources {
            let joined_sources = format_addresses(allowed_sources);
            registry.write_string(ConfigurationRegistryKeys::AllowedSources, &joined_sources)?;
            log::debug!("Set allowed sources to {joined_sources:?}");
        }

        Ok(())
    }
}

/// Parses the `ip_addresses` registry value. Versions before 0.4.0 split it on commas and skipped anything
/// that wasn't an address – and never enforced the result – so hand-edited values such as `a,,b`, `a;b`,
/// `localhost`, CIDR ranges and multi-string values all ran. Rather than refusing to start on them after an
/// upgrade, this also accepts semicolons and whitespace (including the line breaks a multi-string value is
/// read with) as separators, and skips anything else that isn't an address with a warning. That can only
/// widen the interfaces connections are accepted on, never the clients – `allowed_sources` is parsed strictly.
#[cfg(windows)]
fn parse_registry_addresses(string: &str) -> Vec<IpAddr> {
    string
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|item| !item.is_empty())
        .filter_map(|item| match item.parse() {
            Ok(address) => Some(address),
            Err(_) => {
                log::warn!(
                    "Ignoring {item:?} in the ip_addresses registry value, because it isn't an IP address"
                );
                None
            }
        })
        .collect()
}

/// A new installation accepts connections from any client on every interface, so each one gets its own
/// random secret – a shared default secret would let anyone on the network shut it down.
impl Default for AppConfiguration {
    fn default() -> Self {
        AppConfiguration {
            port_number: 53632,
            addresses: Vec::new(),
            secret: generate_secret(),
            allowed_sources: Vec::new(),
        }
    }
}

/// Generates a secret from 128 random bits, hex-encoded so that it's easy to type into a control system.
fn generate_secret() -> String {
    let mut bytes = [0u8; 16];
    // Reads the operating system's cryptographically secure random number generator
    getrandom::fill(&mut bytes).expect("the system random number generator is unavailable");
    bytes.iter().map(|byte| format!("{:02x}", byte)).collect()
}

// Implemented by hand so the secret never ends up in a log
impl std::fmt::Debug for AppConfiguration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppConfiguration")
            .field("port_number", &self.port_number)
            .field("addresses", &self.addresses)
            .field("secret", &"<redacted>")
            .field("allowed_sources", &self.allowed_sources)
            .finish()
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
pub enum ConfigurationRegistryKeys {
    IpAddress,
    Port,
    Secret,
    AllowedSources,
}

#[cfg(windows)]
impl ConfigurationRegistryKeys {
    fn as_str(&self) -> &'static str {
        match self {
            ConfigurationRegistryKeys::IpAddress => "ip_addresses",
            ConfigurationRegistryKeys::Port => "port",
            ConfigurationRegistryKeys::Secret => "secret",
            ConfigurationRegistryKeys::AllowedSources => "allowed_sources",
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
struct Registry {
    root_key: RegKey,
}

#[cfg(windows)]
impl Registry {
    fn with_default_root_key() -> Result<Registry, ConfigurationError> {
        Registry::with_root_key(
            winreg::enums::HKEY_LOCAL_MACHINE,
            PathBuf::from("SOFTWARE").join("ShutdownOnLan"),
        )
    }

    fn with_root_key(
        predefined_key: winreg::HKEY,
        path: PathBuf,
    ) -> Result<Registry, ConfigurationError> {
        let (key, disposition) = RegKey::predef(predefined_key)
            .create_subkey(&path)
            .map_err(ConfigurationError::RegistryUnavailable)?;

        match disposition {
            RegDisposition::REG_CREATED_NEW_KEY => {
                log::info!("Created New Registry Key");
            }
            RegDisposition::REG_OPENED_EXISTING_KEY => {
                log::info!("Using Existing Registry Key");
            }
        }

        Ok(Registry { root_key: key })
    }

    /// Lets only SYSTEM and Administrators open the key. It holds the secret, and keys under
    /// `HKEY_LOCAL_MACHINE\SOFTWARE` otherwise inherit read access for every user.
    fn restrict_access(&self) -> Result<(), ConfigurationError> {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
        };
        use windows_sys::Win32::Security::{
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        };
        use windows_sys::Win32::System::Registry::RegSetKeySecurity;

        // Full control for SYSTEM and Administrators, without inheriting anything from the parent key
        let sddl: Vec<u16> = "D:P(A;OICI;KA;;;SY)(A;OICI;KA;;;BA)"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();

        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        };
        if converted == 0 {
            return Err(ConfigurationError::RegistryAccessNotRestricted(
                std::io::Error::last_os_error(),
            ));
        }

        let status = unsafe {
            RegSetKeySecurity(
                self.root_key.raw_handle() as _,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor,
            )
        };
        unsafe { LocalFree(descriptor as _) };

        if status != 0 {
            return Err(ConfigurationError::RegistryAccessNotRestricted(
                std::io::Error::from_raw_os_error(status as i32),
            ));
        }

        Ok(())
    }

    /// Whether `key` has a value, of any type. A value that exists but can't be read – because it has the
    /// wrong type, for instance – counts, so that it isn't mistaken for a missing one and overwritten. It's
    /// reported when the configuration is read instead, and `set` can replace it.
    fn contains(&self, key: ConfigurationRegistryKeys) -> Result<bool, ConfigurationError> {
        match self.root_key.get_raw_value(key) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_error) => Err(ConfigurationError::RegistryKeyNotReadable(key)),
        }
    }

    fn read_string(&self, key: ConfigurationRegistryKeys) -> Result<String, ConfigurationError> {
        self.root_key
            .get_value(key)
            .map_err(|_error| ConfigurationError::RegistryKeyNotReadable(key))
    }

    fn read_u16(&self, key: ConfigurationRegistryKeys) -> Result<u16, ConfigurationError> {
        use std::convert::TryFrom;
        let value = self.read_u32(key)?;
        u16::try_from(value).map_err(|_error| ConfigurationError::RegistryKeyNotReadable(key))
    }

    fn read_u32(&self, key: ConfigurationRegistryKeys) -> Result<u32, ConfigurationError> {
        self.root_key
            .get_value(key)
            .map_err(|_error| ConfigurationError::RegistryKeyNotReadable(key))
    }

    fn write_string(
        &self,
        key: ConfigurationRegistryKeys,
        value: &String,
    ) -> Result<(), ConfigurationError> {
        self.root_key
            .set_value(key, value)
            .map_err(|_error| ConfigurationError::RegistryKeyNotWritable(key))
    }

    fn write_u32(
        &self,
        key: ConfigurationRegistryKeys,
        value: u32,
    ) -> Result<(), ConfigurationError> {
        self.root_key
            .set_value(key, &value)
            .map_err(|_error| ConfigurationError::RegistryKeyNotWritable(key))
    }
}

/// The preferences domain – the same as the launchd label, so the settings live in
/// `/Library/Preferences/com.jkmassel.shutdownonlan.plist`, and can be managed with a configuration profile.
#[cfg(target_os = "macos")]
const PREFERENCES_DOMAIN: &str = "com.jkmassel.shutdownonlan";

#[cfg(target_os = "macos")]
const PREFERENCES_FILE: &str = "/Library/Preferences/com.jkmassel.shutdownonlan.plist";

/// Where the secret is stored – see `Storage`.
#[cfg(target_os = "macos")]
const SECRET_FILE: &str = "/Library/Application Support/ShutdownOnLan/secret";

/// Where versions before the move to `CFPreferences` stored their configuration.
#[cfg(target_os = "macos")]
const LEGACY_CONFIGURATION_FILE: &str =
    "/Library/Application Support/ShutdownOnLan/ShutDownOnLan.plist";

/// The same names that the legacy plist and the Windows registry values use.
#[cfg(target_os = "macos")]
struct PreferenceKeys {}

#[cfg(target_os = "macos")]
impl PreferenceKeys {
    const PORT: &'static str = "port_number";
    const ADDRESSES: &'static str = "addresses";
    const SECRET: &'static str = "secret";
    const ALLOWED_SOURCES: &'static str = "allowed_sources";
}

#[cfg(target_os = "macos")]
fn property_list_to_u16(value: CFPropertyList) -> Option<u16> {
    let number = value.downcast_into::<CFNumber>()?.to_i64()?;
    u16::try_from(number).ok()
}

#[cfg(target_os = "macos")]
fn property_list_to_addresses(value: CFPropertyList) -> Option<Vec<IpAddr>> {
    value
        .downcast_into::<CFArray>()?
        .get_all_values()
        .into_iter()
        .map(|item| {
            let item = unsafe { CFType::wrap_under_get_rule(item) };
            item.downcast::<CFString>()?.to_string().parse().ok()
        })
        .collect()
}

/// Parses a property list file whose root is a dictionary.
#[cfg(target_os = "macos")]
fn parse_dictionary(bytes: &[u8]) -> Option<CFDictionary> {
    let (plist, _format) = core_foundation::propertylist::create_with_data(
        CFData::from_buffer(bytes),
        core_foundation::propertylist::kCFPropertyListImmutable,
    )
    .ok()?;

    unsafe { CFPropertyList::wrap_under_create_rule(plist) }.downcast_into::<CFDictionary>()
}

#[cfg(target_os = "macos")]
fn addresses_to_property_list(addresses: &[IpAddr]) -> CFPropertyList {
    let strings: Vec<CFString> = addresses
        .iter()
        .map(|ip| CFString::new(&ip.to_string()))
        .collect();
    CFArray::from_CFTypes(&strings)
        .into_untyped()
        .into_CFPropertyList()
}

/// Where the macOS configuration lives: the settings in a preferences domain, and the secret in a file only
/// root can read. The preferences domain can't hold the secret, because `cfprefsd` rewrites its file as
/// readable by every user whenever it flushes changes – and so does running `defaults` on it.
#[cfg(target_os = "macos")]
struct Storage {
    preferences: Preferences,
    /// Where `cfprefsd` stores `preferences`
    preferences_file: PathBuf,
    secret_file: PathBuf,
}

#[cfg(target_os = "macos")]
impl Storage {
    fn system() -> Storage {
        Storage {
            preferences: Preferences::system(),
            preferences_file: PathBuf::from(PREFERENCES_FILE),
            secret_file: PathBuf::from(SECRET_FILE),
        }
    }

    /// Fails if the preferences file exists but can't be parsed. `cfprefsd` treats a file it can't parse as
    /// empty, so every value would read back as missing. Filling them in with defaults would then replace
    /// the admin's port, addresses and allowed sources – and the default allowed sources accept any client.
    /// Any write to the domain would replace the file too, so this is checked before anything is written.
    fn check_preferences_file(&self) -> Result<(), ConfigurationError> {
        let corrupt = || ConfigurationError::CorruptPreferencesFile {
            path: self.preferences_file.display().to_string(),
        };

        match std::fs::read(&self.preferences_file) {
            Ok(bytes) => parse_dictionary(&bytes).map(|_| ()).ok_or_else(corrupt),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                Err(ConfigurationError::RequiresRoot)
            }
            Err(error) => Err(ConfigurationError::PreferencesNotReadable {
                source: error,
                path: self.preferences_file.display().to_string(),
            }),
        }
    }

    /// The secret, or `None` if there isn't one yet. A secret managed by a configuration profile takes
    /// precedence over the file.
    fn read_secret(&self) -> Result<Option<String>, ConfigurationError> {
        if self.preferences.is_forced(PreferenceKeys::SECRET) {
            let secret = self
                .preferences
                .get(PreferenceKeys::SECRET)
                .and_then(|value| value.downcast_into::<CFString>())
                .ok_or(ConfigurationError::PreferenceNotReadable(
                    PreferenceKeys::SECRET,
                ))?;
            return Ok(Some(secret.to_string()));
        }

        match std::fs::read_to_string(&self.secret_file) {
            // Allow for a trailing newline, in case the file was written by hand
            Ok(contents) => Ok(Some(contents.trim_end_matches(['\r', '\n']).to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                Err(ConfigurationError::RequiresRoot)
            }
            Err(error) => Err(ConfigurationError::SecretNotReadable {
                source: error,
                path: self.secret_file.display().to_string(),
            }),
        }
    }

    fn write_secret(&self, secret: &str) -> Result<(), ConfigurationError> {
        let not_writable = |error: std::io::Error| {
            if error.kind() == std::io::ErrorKind::PermissionDenied {
                ConfigurationError::RequiresRoot
            } else {
                ConfigurationError::ConfigurationFileUnwritable {
                    source: error,
                    path: self.secret_file.display().to_string(),
                }
            }
        };

        if let Some(directory) = self.secret_file.parent() {
            std::fs::create_dir_all(directory).map_err(not_writable)?;
        }

        write_private_file(&self.secret_file, secret.as_bytes()).map_err(not_writable)
    }
}

/// A preferences domain. Reads go through the standard search list, so values managed by a configuration
/// profile take precedence. Writes go to `user`, for any host.
#[cfg(target_os = "macos")]
struct Preferences {
    application_id: CFString,
    user: CFStringRef,
}

#[cfg(target_os = "macos")]
impl Preferences {
    /// The system-wide domain the service reads. Writing to it requires root.
    fn system() -> Preferences {
        Preferences {
            application_id: CFString::new(PREFERENCES_DOMAIN),
            user: unsafe { core_foundation_sys::preferences::kCFPreferencesAnyUser },
        }
    }

    fn get(&self, key: &str) -> Option<CFPropertyList> {
        let key = CFString::new(key);
        let value = unsafe {
            core_foundation_sys::preferences::CFPreferencesCopyAppValue(
                key.as_concrete_TypeRef(),
                self.application_id.as_concrete_TypeRef(),
            )
        };

        Self::wrap(value)
    }

    /// Like `get`, but only looks in the domain that `set` writes to.
    fn get_local(&self, key: &str) -> Option<CFPropertyList> {
        let key = CFString::new(key);
        let value = unsafe {
            core_foundation_sys::preferences::CFPreferencesCopyValue(
                key.as_concrete_TypeRef(),
                self.application_id.as_concrete_TypeRef(),
                self.user,
                core_foundation_sys::preferences::kCFPreferencesAnyHost,
            )
        };

        Self::wrap(value)
    }

    fn wrap(value: core_foundation_sys::propertylist::CFPropertyListRef) -> Option<CFPropertyList> {
        if value.is_null() {
            None
        } else {
            Some(unsafe { CFPropertyList::wrap_under_create_rule(value) })
        }
    }

    fn set(&self, key: &str, value: &CFPropertyList) {
        self.set_raw(key, value.as_concrete_TypeRef());
    }

    fn remove(&self, key: &str) {
        self.set_raw(key, std::ptr::null());
    }

    fn set_raw(&self, key: &str, value: core_foundation_sys::propertylist::CFPropertyListRef) {
        let key = CFString::new(key);
        unsafe {
            core_foundation_sys::preferences::CFPreferencesSetValue(
                key.as_concrete_TypeRef(),
                value,
                self.application_id.as_concrete_TypeRef(),
                self.user,
                core_foundation_sys::preferences::kCFPreferencesAnyHost,
            )
        }
    }

    /// Whether `key` is managed by a configuration profile.
    fn is_forced(&self, key: &str) -> bool {
        let key = CFString::new(key);
        unsafe {
            core_foundation_sys::preferences::CFPreferencesAppValueIsForced(
                key.as_concrete_TypeRef(),
                self.application_id.as_concrete_TypeRef(),
            ) != 0
        }
    }

    /// Hands pending changes to `cfprefsd`. This is where writing without permission fails.
    fn synchronize(&self) -> Result<(), ConfigurationError> {
        let succeeded = unsafe {
            core_foundation_sys::preferences::CFPreferencesSynchronize(
                self.application_id.as_concrete_TypeRef(),
                self.user,
                core_foundation_sys::preferences::kCFPreferencesAnyHost,
            ) != 0
        };

        if succeeded {
            Ok(())
        } else {
            Err(ConfigurationError::RequiresRoot)
        }
    }
}

/// Replaces the contents of `path` with `contents`, leaving the file readable only by its owner – it
/// holds the secret, which any local user could otherwise read.
///
/// The contents are written to a temporary file that then replaces `path`, so a crash or a full disk
/// part-way through leaves the previous contents in place rather than an empty or truncated file.
#[cfg(unix)]
fn write_private_file(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut temporary_name = std::ffi::OsString::from(".");
    temporary_name.push(path.file_name().unwrap_or_default());
    temporary_name.push(".tmp");
    let temporary_path = path.with_file_name(temporary_name);

    let result = (|| {
        // Truncate a temporary file left behind by an earlier attempt
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary_path)?;

        // `mode` only applies when the file is created, so also restrict a leftover temporary file
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(contents)?;
        file.sync_all()?;

        std::fs::rename(&temporary_path, path)
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }

    result
}

#[derive(Error, Debug)]
pub enum ConfigurationError {
    #[error("No Configuration File at Path")]
    MissingConfigurationFile(#[from] std::io::Error),

    #[cfg(target_os = "linux")]
    #[error("Unable to read the configuration file")]
    InvalidConfigurationFile { source: std::io::Error },

    #[cfg(target_os = "macos")]
    #[error("Contents of Configuration File Are Invalid")]
    CorruptConfigurationFile,

    #[cfg(target_os = "macos")]
    #[error("The {0} preference is missing or invalid")]
    PreferenceNotReadable(&'static str),

    #[cfg(target_os = "macos")]
    #[error(
        "{path} isn't a valid property list, so the configuration can't be read. Fix or delete it, then try again – deleting it restores the default settings."
    )]
    CorruptPreferencesFile { path: String },

    #[cfg(target_os = "macos")]
    #[error("Unable to read {path}")]
    PreferencesNotReadable {
        source: std::io::Error,
        path: String,
    },

    #[cfg(not(windows))]
    #[error("Only root can read or change the configuration – try again with sudo")]
    RequiresRoot,

    #[cfg(target_os = "macos")]
    #[error("Unable to read the secret from {path}")]
    SecretNotReadable {
        source: std::io::Error,
        path: String,
    },

    #[cfg(target_os = "macos")]
    #[error(
        "The {0} preference is managed by a configuration profile, so it can't be changed here"
    )]
    PreferenceIsManaged(&'static str),

    #[cfg(target_os = "linux")]
    #[error("Contents of Configuration File Are Invalid")]
    CorruptTomlConfigurationFile(#[source] toml::de::Error),

    #[cfg(target_os = "linux")]
    #[error("The configuration file in memory can't be converted to an on-disk representation")]
    InvalidConfiguration,

    #[error("The secret must be between 1 and {} bytes long", MAX_SECRET_LENGTH)]
    InvalidSecret,

    #[error("The port must be between 1 and 65535")]
    InvalidPort,

    #[error(transparent)]
    InvalidAddress(#[from] AddrParseError),

    #[error(
        "{0} isn't a client address, so it would never match – to allow any client, leave the allowed sources empty"
    )]
    UnspecifiedSource(IpAddr),

    #[cfg(windows)]
    #[error("Unable to open the configuration registry key")]
    RegistryUnavailable(#[source] std::io::Error),

    #[cfg(windows)]
    #[error("Unable to restrict access to the configuration registry key")]
    RegistryAccessNotRestricted(#[source] std::io::Error),

    #[cfg(windows)]
    #[error("Unable to read registry value {0:?}")]
    RegistryKeyNotReadable(ConfigurationRegistryKeys),

    #[cfg(windows)]
    #[error("Unable to write registry value {0:?}")]
    RegistryKeyNotWritable(ConfigurationRegistryKeys),

    #[cfg(target_os = "linux")]
    #[error("Unable to write to configuration storage directory")]
    ConfigurationStorageUnwritable {
        source: std::io::Error,
        path: String,
    },

    #[cfg(not(windows))]
    #[error("Unable to write to configuration file at {path}")]
    ConfigurationFileUnwritable {
        source: std::io::Error,
        path: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies changes to a configuration in memory, the way `set` would to storage.
    trait Set {
        fn set_addresses(&mut self, string: &str) -> Result<(), ConfigurationError>;
        fn set_allowed_sources(&mut self, string: &str) -> Result<(), ConfigurationError>;
        fn set_secret(&mut self, secret: String) -> Result<(), ConfigurationError>;
    }

    impl Set for AppConfiguration {
        fn set_addresses(&mut self, string: &str) -> Result<(), ConfigurationError> {
            let mut update = ConfigurationUpdate::default();
            update.set_addresses(string)?;
            self.addresses = update.addresses.unwrap();
            Ok(())
        }

        fn set_allowed_sources(&mut self, string: &str) -> Result<(), ConfigurationError> {
            let mut update = ConfigurationUpdate::default();
            update.set_allowed_sources(string)?;
            self.allowed_sources = update.allowed_sources.unwrap();
            Ok(())
        }

        fn set_secret(&mut self, secret: String) -> Result<(), ConfigurationError> {
            let mut update = ConfigurationUpdate::default();
            update.set_secret(secret)?;
            self.secret = update.secret.unwrap();
            Ok(())
        }
    }

    #[test]
    fn test_port_zero_cannot_be_set() {
        let mut update = ConfigurationUpdate::default();
        assert!(matches!(
            update.set_port(0),
            Err(ConfigurationError::InvalidPort)
        ));
        update.set_port(1).unwrap();
        assert_eq!(update.port_number, Some(1));
    }

    #[test]
    fn test_set_addresses_accepts_a_comma_separated_list() {
        let mut configuration = AppConfiguration::default();
        configuration.set_addresses("10.0.1.100, ::1").unwrap();

        assert_eq!(
            configuration.addresses,
            vec![
                "10.0.1.100".parse::<IpAddr>().unwrap(),
                "::1".parse::<IpAddr>().unwrap()
            ]
        );
    }

    #[test]
    fn test_set_addresses_rejects_invalid_addresses_without_modifying_the_configuration() {
        let mut configuration = AppConfiguration::default();
        configuration.set_addresses("10.0.1.100").unwrap();

        assert!(
            configuration
                .set_addresses("10.0.1.100,10.0.1.300")
                .is_err()
        );
        assert_eq!(
            configuration.addresses,
            vec!["10.0.1.100".parse::<IpAddr>().unwrap()]
        );

        configuration.set_addresses("").unwrap();
        assert!(configuration.addresses.is_empty());
    }

    #[test]
    fn test_empty_allowed_sources_accepts_any_client() {
        let mut configuration = AppConfiguration::default();
        configuration.set_allowed_sources("").unwrap();

        assert!(configuration.accepts_connections_from(&"10.0.1.50".parse().unwrap()));
    }

    #[test]
    fn test_allowed_sources_only_accepts_listed_clients() {
        let mut configuration = AppConfiguration::default();
        configuration
            .set_allowed_sources("10.0.1.50, 10.0.1.51")
            .unwrap();

        assert!(configuration.accepts_connections_from(&"10.0.1.51".parse().unwrap()));
        assert!(!configuration.accepts_connections_from(&"10.0.1.52".parse().unwrap()));
        assert!(configuration.set_allowed_sources("10.0.1.300").is_err());
    }

    #[test]
    fn test_ipv4_mapped_addresses_match_ipv4_addresses() {
        let mut configuration = AppConfiguration::default();
        configuration.set_addresses("10.0.1.100").unwrap();
        configuration
            .set_allowed_sources("::ffff:10.0.1.50")
            .unwrap();

        assert!(configuration.accepts_connections_on(&"::ffff:10.0.1.100".parse().unwrap()));
        assert!(configuration.accepts_connections_from(&"10.0.1.50".parse().unwrap()));
        assert!(!configuration.accepts_connections_from(&"::1".parse().unwrap()));
    }

    #[test]
    fn test_unspecified_addresses_accept_connections_on_every_interface() {
        let mut configuration = AppConfiguration::default();

        configuration.set_addresses("0.0.0.0").unwrap();
        assert!(configuration.accepts_connections_on(&"10.0.1.100".parse().unwrap()));
        assert!(configuration.accepts_connections_on(&"::ffff:10.0.1.100".parse().unwrap()));
        assert!(!configuration.accepts_connections_on(&"fe80::1".parse().unwrap()));

        configuration.set_addresses("::").unwrap();
        assert!(configuration.accepts_connections_on(&"10.0.1.100".parse().unwrap()));
        assert!(configuration.accepts_connections_on(&"fe80::1".parse().unwrap()));
    }

    #[test]
    fn test_unspecified_addresses_are_not_allowed_sources() {
        let mut configuration = AppConfiguration::default();

        for sources in ["0.0.0.0", "10.0.1.50, ::"] {
            assert!(matches!(
                configuration.set_allowed_sources(sources),
                Err(ConfigurationError::UnspecifiedSource(_))
            ));
        }
        assert!(configuration.allowed_sources.is_empty());

        // For instance, written by hand
        configuration.allowed_sources = vec!["0.0.0.0".parse().unwrap()];
        assert!(configuration.validate().is_err());
    }

    #[test]
    fn test_port_zero_is_invalid() {
        let configuration = AppConfiguration {
            port_number: 0,
            ..AppConfiguration::default()
        };

        assert!(matches!(
            configuration.validate(),
            Err(ConfigurationError::InvalidPort)
        ));
        assert!(AppConfiguration::default().validate().is_ok());
    }

    #[test]
    fn test_set_secret_enforces_length_limits() {
        let mut configuration = AppConfiguration::default();

        assert!(configuration.set_secret(String::new()).is_err());
        assert!(
            configuration
                .set_secret("a".repeat(MAX_SECRET_LENGTH + 1))
                .is_err()
        );
        assert!(
            configuration
                .set_secret("a".repeat(MAX_SECRET_LENGTH))
                .is_ok()
        );
    }

    #[test]
    fn test_default_configuration_accepts_connections_on_every_interface() {
        let configuration = AppConfiguration::default();

        assert!(configuration.accepts_connections_on(&"127.0.0.1".parse().unwrap()));
        assert!(configuration.accepts_connections_on(&"10.0.1.100".parse().unwrap()));
    }

    #[test]
    fn test_addresses_only_accept_connections_on_listed_interfaces() {
        let mut configuration = AppConfiguration::default();
        configuration.set_addresses("10.0.1.100").unwrap();

        assert!(configuration.accepts_connections_on(&"10.0.1.100".parse().unwrap()));
        assert!(!configuration.accepts_connections_on(&"192.168.1.100".parse().unwrap()));
    }

    #[test]
    fn test_each_default_configuration_has_its_own_random_secret() {
        let first = AppConfiguration::default().secret;
        let second = AppConfiguration::default().secret;

        assert_ne!(first, second);
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_legacy_default_secret_is_detected() {
        let mut configuration = AppConfiguration::default();
        assert!(!configuration.uses_legacy_default_secret());

        configuration
            .set_secret(LEGACY_DEFAULT_SECRET.to_string())
            .unwrap();
        assert!(configuration.uses_legacy_default_secret());
    }

    #[test]
    fn test_debug_output_does_not_include_the_secret() {
        let configuration = AppConfiguration::default();
        let output = format!("{:?}", configuration);
        assert!(!output.contains(&configuration.secret));
    }

    #[cfg(unix)]
    #[test]
    fn test_configuration_file_is_only_readable_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let path =
            std::env::temp_dir().join(format!("shutdown-on-lan-test-{}.mode", std::process::id()));
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;

        write_private_file(&path, b"first").unwrap();
        assert_eq!(mode(&path), 0o600);

        // A file left readable by an older version is restricted the next time it's written
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private_file(&path, b"second").unwrap();
        let contents = std::fs::read(&path).unwrap();
        let final_mode = mode(&path);
        std::fs::remove_file(&path).unwrap();

        assert_eq!(final_mode, 0o600);
        assert_eq!(contents, b"second");
    }

    #[cfg(unix)]
    #[test]
    fn test_writing_a_file_leaves_no_temporary_file_behind() {
        let directory = std::env::temp_dir().join(format!(
            "shutdown-on-lan-test-{}-atomic",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("configuration");

        write_private_file(&path, b"a longer first version").unwrap();
        write_private_file(&path, b"second").unwrap();

        let contents = std::fs::read(&path).unwrap();
        let entries = std::fs::read_dir(&directory).unwrap().count();
        std::fs::remove_dir_all(&directory).unwrap();

        assert_eq!(contents, b"second");
        assert_eq!(entries, 1);
    }

    /// Storage in a temporary directory (so no root is needed) that's deleted when the test finishes. The
    /// preferences domain is identified by a path in the directory, so `cfprefsd` stores it there too.
    #[cfg(target_os = "macos")]
    struct TestStorage {
        storage: Storage,
        directory: PathBuf,
    }

    #[cfg(target_os = "macos")]
    impl TestStorage {
        fn new(name: &str) -> TestStorage {
            let directory = std::env::temp_dir().join(format!(
                "shutdown-on-lan-test-{}-{}",
                std::process::id(),
                name
            ));
            std::fs::create_dir_all(&directory).unwrap();
            let domain = directory.join(PREFERENCES_DOMAIN);

            TestStorage {
                storage: Storage {
                    preferences_file: directory.join(format!("{PREFERENCES_DOMAIN}.plist")),
                    preferences: Preferences {
                        application_id: CFString::new(domain.to_str().unwrap()),
                        user: unsafe {
                            core_foundation_sys::preferences::kCFPreferencesCurrentUser
                        },
                    },
                    secret_file: directory.join("ShutdownOnLan").join("secret"),
                },
                directory,
            }
        }

        fn preferences(&self) -> &Preferences {
            &self.storage.preferences
        }

        fn legacy_file(&self, contents: &str) -> PathBuf {
            let path = self.directory.join("ShutDownOnLan.plist");
            std::fs::write(&path, contents).unwrap();
            path
        }

        fn custom_configuration() -> AppConfiguration {
            let mut configuration = AppConfiguration {
                port_number: 12345,
                ..AppConfiguration::default()
            };
            configuration.set_addresses("10.0.1.100,::1").unwrap();
            configuration
                .set_secret("custom secret".to_string())
                .unwrap();
            configuration.set_allowed_sources("10.0.1.50").unwrap();
            configuration
        }
    }

    #[cfg(target_os = "macos")]
    impl AppConfiguration {
        fn save_to(&self, storage: &Storage) -> Result<(), ConfigurationError> {
            ConfigurationUpdate::from(self).apply_to(storage)
        }
    }

    #[cfg(target_os = "macos")]
    impl Drop for TestStorage {
        fn drop(&mut self) {
            for key in [
                PreferenceKeys::PORT,
                PreferenceKeys::ADDRESSES,
                PreferenceKeys::SECRET,
                PreferenceKeys::ALLOWED_SOURCES,
            ] {
                self.preferences().remove(key);
            }
            let _ = self.preferences().synchronize();
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    #[cfg(target_os = "macos")]
    const LEGACY_PLIST_WITHOUT_ALLOWED_SOURCES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>port_number</key>
	<integer>12345</integer>
	<key>addresses</key>
	<array>
		<string>10.0.1.100</string>
		<string>::1</string>
	</array>
	<key>secret</key>
	<string>custom secret</string>
</dict>
</plist>"#;

    #[cfg(target_os = "macos")]
    #[test]
    fn test_storage_round_trip() {
        use std::os::unix::fs::PermissionsExt;

        let test = TestStorage::new("round-trip");
        let configuration = TestStorage::custom_configuration();

        configuration.save_to(&test.storage).unwrap();

        assert_eq!(
            AppConfiguration::fetch_from(&test.storage).unwrap(),
            configuration
        );

        // The secret is only stored in the private file
        assert!(test.preferences().get(PreferenceKeys::SECRET).is_none());
        let metadata = std::fs::metadata(&test.storage.secret_file).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_empty_storage_is_filled_with_defaults() {
        let test = TestStorage::new("empty");

        AppConfiguration::write_missing_defaults(&test.storage).unwrap();

        // Every default configuration has a different random secret, so compare everything else
        let configuration = AppConfiguration::fetch_from(&test.storage).unwrap();
        assert_eq!(configuration.secret.len(), 32);
        assert_eq!(
            configuration,
            AppConfiguration {
                secret: configuration.secret.clone(),
                ..AppConfiguration::default()
            }
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_only_missing_values_are_restored() {
        let test = TestStorage::new("missing-value");
        let configuration = TestStorage::custom_configuration();

        configuration.save_to(&test.storage).unwrap();
        test.preferences().remove(PreferenceKeys::PORT);

        AppConfiguration::write_missing_defaults(&test.storage).unwrap();

        assert_eq!(
            AppConfiguration::fetch_from(&test.storage).unwrap(),
            AppConfiguration {
                port_number: AppConfiguration::default().port_number,
                ..configuration
            }
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_preferences_are_stored_in_the_expected_file() {
        let test = TestStorage::new("preferences-file");

        TestStorage::custom_configuration()
            .save_to(&test.storage)
            .unwrap();

        assert!(test.storage.preferences_file.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_corrupt_preferences_file_is_not_replaced_with_defaults() {
        let test = TestStorage::new("corrupt-preferences");
        std::fs::write(&test.storage.preferences_file, "not a plist").unwrap();
        let legacy_file = test.directory.join("missing.plist");

        assert!(matches!(
            AppConfiguration::prepare(&test.storage, &legacy_file),
            Err(ConfigurationError::CorruptPreferencesFile { .. })
        ));
        assert!(matches!(
            TestStorage::custom_configuration().save_to(&test.storage),
            Err(ConfigurationError::CorruptPreferencesFile { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(&test.storage.preferences_file).unwrap(),
            "not a plist"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_invalid_preferences_are_reported() {
        let test = TestStorage::new("invalid-value");
        TestStorage::custom_configuration()
            .save_to(&test.storage)
            .unwrap();

        // The port should be a number
        test.preferences().set(
            PreferenceKeys::PORT,
            &CFString::new("not a number").into_CFPropertyList(),
        );

        assert!(matches!(
            AppConfiguration::fetch_from(&test.storage),
            Err(ConfigurationError::PreferenceNotReadable(
                PreferenceKeys::PORT
            ))
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_update_replaces_an_invalid_value_and_leaves_the_rest() {
        let test = TestStorage::new("update-invalid-value");
        let configuration = TestStorage::custom_configuration();
        configuration.save_to(&test.storage).unwrap();

        test.preferences().set(
            PreferenceKeys::PORT,
            &CFString::new("not a number").into_CFPropertyList(),
        );
        std::fs::write(&test.storage.secret_file, "").unwrap();
        assert!(AppConfiguration::fetch_from(&test.storage).is_err());

        let mut update = ConfigurationUpdate::default();
        update.set_port(4321).unwrap();
        update.set_secret("new secret".to_string()).unwrap();
        update.apply_to(&test.storage).unwrap();

        assert_eq!(
            AppConfiguration::fetch_from(&test.storage).unwrap(),
            AppConfiguration {
                port_number: 4321,
                secret: "new secret".to_string(),
                ..configuration
            }
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_secret_file_written_by_hand_is_read() {
        let test = TestStorage::new("secret-by-hand");
        TestStorage::custom_configuration()
            .save_to(&test.storage)
            .unwrap();

        std::fs::write(&test.storage.secret_file, "typed secret\n").unwrap();

        assert_eq!(
            AppConfiguration::fetch_from(&test.storage).unwrap().secret,
            "typed secret"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_secret_in_preferences_is_moved_to_the_secret_file() {
        let test = TestStorage::new("secret-in-preferences");
        TestStorage::custom_configuration()
            .save_to(&test.storage)
            .unwrap();

        // For instance, with `defaults write`
        test.preferences().set(
            PreferenceKeys::SECRET,
            &CFString::new("written with defaults").into_CFPropertyList(),
        );
        test.preferences().synchronize().unwrap();

        AppConfiguration::migrate_secret_from_preferences(&test.storage).unwrap();

        assert_eq!(
            AppConfiguration::fetch_from(&test.storage).unwrap().secret,
            "written with defaults"
        );
        assert!(test.preferences().get(PreferenceKeys::SECRET).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_invalid_secret_in_preferences_is_not_moved() {
        let test = TestStorage::new("empty-secret-in-preferences");
        TestStorage::custom_configuration()
            .save_to(&test.storage)
            .unwrap();

        test.preferences().set(
            PreferenceKeys::SECRET,
            &CFString::new("").into_CFPropertyList(),
        );

        assert!(matches!(
            AppConfiguration::migrate_secret_from_preferences(&test.storage),
            Err(ConfigurationError::InvalidSecret)
        ));
        assert_eq!(
            AppConfiguration::fetch_from(&test.storage).unwrap().secret,
            "custom secret"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_legacy_file_is_migrated_and_removed() {
        let test = TestStorage::new("migrate");
        let path = test.legacy_file(LEGACY_PLIST_WITHOUT_ALLOWED_SOURCES);

        AppConfiguration::migrate_legacy_file(&path, &test.storage).unwrap();

        let migrated = AppConfiguration::fetch_from(&test.storage).unwrap();
        assert_eq!(
            migrated,
            AppConfiguration {
                allowed_sources: Vec::new(),
                ..TestStorage::custom_configuration()
            }
        );
        assert!(migrated.accepts_connections_from(&"10.0.1.99".parse().unwrap()));
        assert!(test.preferences().get(PreferenceKeys::SECRET).is_none());
        assert!(!path.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_legacy_default_address_accepts_connections_on_every_interface() {
        let test = TestStorage::new("migrate-default-address");
        let path = test.legacy_file(&LEGACY_PLIST_WITHOUT_ALLOWED_SOURCES.replace(
            "<string>10.0.1.100</string>\n\t\t<string>::1</string>",
            "<string>127.0.0.1</string>",
        ));

        AppConfiguration::migrate_legacy_file(&path, &test.storage).unwrap();

        let migrated = AppConfiguration::fetch_from(&test.storage).unwrap();
        assert!(migrated.addresses.is_empty());
        assert!(migrated.accepts_connections_on(&"10.0.1.100".parse().unwrap()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_legacy_file_replaces_existing_values() {
        let test = TestStorage::new("migrate-over-defaults");
        AppConfiguration::write_missing_defaults(&test.storage).unwrap();
        let path = test.legacy_file(LEGACY_PLIST_WITHOUT_ALLOWED_SOURCES);

        AppConfiguration::migrate_legacy_file(&path, &test.storage).unwrap();

        let migrated = AppConfiguration::fetch_from(&test.storage).unwrap();
        assert_eq!(migrated.port_number, 12345);
        assert_eq!(migrated.secret, "custom secret");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_corrupt_legacy_file_is_kept() {
        let test = TestStorage::new("migrate-corrupt");
        let path = test.legacy_file("not a plist");

        assert!(matches!(
            AppConfiguration::migrate_legacy_file(&path, &test.storage),
            Err(ConfigurationError::CorruptConfigurationFile)
        ));
        assert!(path.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_toml_round_trip() {
        let mut configuration = AppConfiguration::default();
        configuration.set_addresses("10.0.1.100,::1").unwrap();
        configuration.set_allowed_sources("10.0.1.50").unwrap();

        let toml = configuration.to_toml().unwrap();
        assert!(toml.contains(r#"addresses = ["10.0.1.100", "::1"]"#));
        assert!(toml.contains(r#"allowed_sources = ["10.0.1.50"]"#));
        assert_eq!(AppConfiguration::from_toml(&toml).unwrap(), configuration);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_misspelled_toml_keys_are_rejected() {
        let hand_written = r#"
            port_number = 53632
            addresses = []
            secret = "a secret"
            allowed_source = ["10.0.1.50"]
        "#;

        assert!(matches!(
            AppConfiguration::from_toml(hand_written),
            Err(ConfigurationError::CorruptTomlConfigurationFile(_))
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_update_replaces_an_invalid_value_and_leaves_the_rest() {
        let hand_written = r#"
            port_number = "not a number"
            addresses = ["10.0.1.100"]
            secret = ""
            allowed_sources = ["10.0.1.50"]
        "#;
        assert!(
            AppConfiguration::from_toml(hand_written)
                .and_then(|configuration| configuration.validate())
                .is_err()
        );

        let mut update = ConfigurationUpdate::default();
        update.set_port(4321).unwrap();
        update.set_secret("new secret".to_string()).unwrap();
        let updated = update.apply_to_toml(hand_written).unwrap();

        let mut expected = AppConfiguration {
            port_number: 4321,
            secret: "new secret".to_string(),
            ..AppConfiguration::default()
        };
        expected.set_addresses("10.0.1.100").unwrap();
        expected.set_allowed_sources("10.0.1.50").unwrap();
        assert_eq!(AppConfiguration::from_toml(&updated).unwrap(), expected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_update_writes_values_that_round_trip() {
        let configuration = AppConfiguration::default();
        let mut update = ConfigurationUpdate::default();
        update.set_addresses("10.0.1.100, ::1").unwrap();
        update.set_allowed_sources("").unwrap();

        let updated = update
            .apply_to_toml(&configuration.to_toml().unwrap())
            .unwrap();

        let mut expected = configuration;
        expected.set_addresses("10.0.1.100,::1").unwrap();
        assert_eq!(AppConfiguration::from_toml(&updated).unwrap(), expected);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_toml_without_allowed_sources_accepts_any_client() {
        let configuration = AppConfiguration::default();

        let toml = configuration.to_toml().unwrap();
        assert_eq!(AppConfiguration::from_toml(&toml).unwrap(), configuration);

        let hand_written = r#"
            port_number = 53632
            addresses = ["127.0.0.1"]
            secret = "Super Secret String"
        "#;
        assert!(
            AppConfiguration::from_toml(hand_written)
                .unwrap()
                .allowed_sources
                .is_empty()
        );
    }

    /// A registry key under `HKEY_CURRENT_USER` (so no admin rights are needed) that's deleted when
    /// the test finishes.
    #[cfg(windows)]
    struct TestRegistry {
        path: PathBuf,
        registry: Registry,
    }

    #[cfg(windows)]
    impl TestRegistry {
        fn new(name: &str) -> TestRegistry {
            let path = PathBuf::from("Software").join(format!(
                "ShutdownOnLan-Test-{}-{}",
                std::process::id(),
                name
            ));
            let registry =
                Registry::with_root_key(winreg::enums::HKEY_CURRENT_USER, path.clone()).unwrap();

            TestRegistry { path, registry }
        }

        fn custom_configuration() -> AppConfiguration {
            let mut configuration = AppConfiguration {
                port_number: 12345,
                ..AppConfiguration::default()
            };
            configuration.set_addresses("10.0.1.100").unwrap();
            configuration
                .set_secret("custom secret".to_string())
                .unwrap();
            configuration.set_allowed_sources("10.0.1.50").unwrap();
            configuration
        }
    }

    #[cfg(windows)]
    impl AppConfiguration {
        fn save_to(&self, registry: &Registry) -> Result<(), ConfigurationError> {
            ConfigurationUpdate::from(self).apply_to(registry)
        }
    }

    #[cfg(windows)]
    impl Drop for TestRegistry {
        fn drop(&mut self) {
            let _ = RegKey::predef(winreg::enums::HKEY_CURRENT_USER).delete_subkey_all(&self.path);
        }
    }

    #[cfg(windows)]
    #[test]
    fn test_registry_round_trip() {
        let test = TestRegistry::new("round-trip");
        let configuration = TestRegistry::custom_configuration();

        configuration.save_to(&test.registry).unwrap();

        assert_eq!(
            AppConfiguration::fetch_from(&test.registry).unwrap(),
            configuration
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_empty_registry_is_filled_with_defaults() {
        let test = TestRegistry::new("empty");

        AppConfiguration::write_missing_defaults(&test.registry).unwrap();

        // Every default configuration has a different random secret, so compare everything else
        let configuration = AppConfiguration::fetch_from(&test.registry).unwrap();
        assert_eq!(configuration.secret.len(), 32);
        assert_eq!(
            configuration,
            AppConfiguration {
                secret: configuration.secret.clone(),
                ..AppConfiguration::default()
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_upgraded_registry_gets_empty_allowed_sources() {
        let test = TestRegistry::new("upgrade");
        let configuration = TestRegistry::custom_configuration();

        // Configurations written by older versions don't have `allowed_sources`
        configuration.save_to(&test.registry).unwrap();
        test.registry
            .root_key
            .delete_value(ConfigurationRegistryKeys::AllowedSources)
            .unwrap();

        AppConfiguration::write_missing_defaults(&test.registry).unwrap();

        assert_eq!(
            test.registry
                .read_string(ConfigurationRegistryKeys::AllowedSources)
                .unwrap(),
            ""
        );

        let upgraded = AppConfiguration::fetch_from(&test.registry).unwrap();
        assert!(upgraded.allowed_sources.is_empty());
        assert!(upgraded.accepts_connections_from(&"10.0.1.99".parse().unwrap()));
        assert_eq!(upgraded.port_number, configuration.port_number);
        assert_eq!(upgraded.addresses, configuration.addresses);
        assert_eq!(upgraded.secret, configuration.secret);
    }

    #[cfg(windows)]
    #[test]
    fn test_upgraded_registry_with_the_legacy_default_address_accepts_connections_on_every_interface()
     {
        let test = TestRegistry::new("upgrade-default-address");

        // As written by 0.3.0
        for (key, value) in [
            (ConfigurationRegistryKeys::IpAddress, "127.0.0.1"),
            (ConfigurationRegistryKeys::Secret, LEGACY_DEFAULT_SECRET),
        ] {
            test.registry.write_string(key, &value.to_string()).unwrap();
        }
        test.registry
            .write_u32(ConfigurationRegistryKeys::Port, 53632)
            .unwrap();

        AppConfiguration::write_missing_defaults(&test.registry).unwrap();

        let upgraded = AppConfiguration::fetch_from(&test.registry).unwrap();
        assert!(upgraded.addresses.is_empty());
        assert!(upgraded.uses_legacy_default_secret());

        // Once upgraded, a configuration that's deliberately set to 127.0.0.1 is left alone
        test.registry
            .write_string(
                ConfigurationRegistryKeys::IpAddress,
                &"127.0.0.1".to_string(),
            )
            .unwrap();
        AppConfiguration::write_missing_defaults(&test.registry).unwrap();
        assert_eq!(
            AppConfiguration::fetch_from(&test.registry)
                .unwrap()
                .addresses,
            vec![LEGACY_DEFAULT_ADDRESS]
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_only_missing_registry_values_are_restored() {
        let test = TestRegistry::new("missing-value");
        let configuration = TestRegistry::custom_configuration();

        configuration.save_to(&test.registry).unwrap();
        test.registry
            .root_key
            .delete_value(ConfigurationRegistryKeys::Port)
            .unwrap();

        AppConfiguration::write_missing_defaults(&test.registry).unwrap();

        assert_eq!(
            AppConfiguration::fetch_from(&test.registry).unwrap(),
            AppConfiguration {
                port_number: AppConfiguration::default().port_number,
                ..configuration
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_unreadable_registry_values_are_not_overwritten() {
        let test = TestRegistry::new("unreadable-value");
        let configuration = TestRegistry::custom_configuration();

        configuration.save_to(&test.registry).unwrap();

        // The port should be a DWORD – a string can't be read as one
        test.registry
            .write_string(ConfigurationRegistryKeys::Port, &"not a number".to_string())
            .unwrap();

        AppConfiguration::write_missing_defaults(&test.registry).unwrap();
        assert_eq!(
            test.registry
                .read_string(ConfigurationRegistryKeys::Port)
                .unwrap(),
            "not a number"
        );
        assert!(AppConfiguration::fetch_from(&test.registry).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn test_update_replaces_an_invalid_value_and_leaves_the_rest() {
        let test = TestRegistry::new("update-invalid-value");
        let configuration = TestRegistry::custom_configuration();
        configuration.save_to(&test.registry).unwrap();

        test.registry
            .write_string(ConfigurationRegistryKeys::Port, &"not a number".to_string())
            .unwrap();
        test.registry
            .write_string(ConfigurationRegistryKeys::Secret, &String::new())
            .unwrap();

        let mut update = ConfigurationUpdate::default();
        update.set_port(4321).unwrap();
        update.set_secret("new secret".to_string()).unwrap();
        update.apply_to(&test.registry).unwrap();

        assert_eq!(
            AppConfiguration::fetch_from(&test.registry).unwrap(),
            AppConfiguration {
                port_number: 4321,
                secret: "new secret".to_string(),
                ..configuration
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn test_registry_addresses_are_parsed_like_older_versions() {
        let address = |ip: &str| ip.parse::<IpAddr>().unwrap();

        assert_eq!(
            parse_registry_addresses("10.0.1.100,,10.0.1.101"),
            vec![address("10.0.1.100"), address("10.0.1.101")]
        );
        assert_eq!(
            parse_registry_addresses("10.0.1.100; ::1"),
            vec![address("10.0.1.100"), address("::1")]
        );
        // As a multi-string value is read
        assert_eq!(
            parse_registry_addresses("10.0.1.100\n::1"),
            vec![address("10.0.1.100"), address("::1")]
        );
        assert_eq!(
            parse_registry_addresses("localhost,10.0.1.0/24,10.0.1.100"),
            vec![address("10.0.1.100")]
        );
        assert!(parse_registry_addresses("localhost").is_empty());
        assert!(parse_registry_addresses("").is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn test_hand_edited_registry_addresses_do_not_stop_the_configuration_loading() {
        let test = TestRegistry::new("hand-edited-addresses");
        TestRegistry::custom_configuration()
            .save_to(&test.registry)
            .unwrap();

        test.registry
            .root_key
            .set_value(
                ConfigurationRegistryKeys::IpAddress,
                &vec!["10.0.1.100", "localhost"],
            )
            .unwrap();

        assert_eq!(
            AppConfiguration::fetch_from(&test.registry)
                .unwrap()
                .addresses,
            vec!["10.0.1.100".parse::<IpAddr>().unwrap()]
        );
    }
}
