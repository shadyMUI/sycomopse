use peripheral_hub_lib::{
    devices::{self, UsbMetadata},
    models::*,
    providers::{self, headsetcontrol, openrazer, ratbag, upower},
};
use serde::Deserialize;
use serde_json::{json, Value};

#[test]
fn usb_association_does_not_choose_first_duplicate_serial_and_verified_wired_hid_merges() {
    let first = usb(
        "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-2",
        Some("SAME"),
        0x1532,
        0x00b8,
        false,
    );
    let second = usb(
        "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-3",
        Some("SAME"),
        0x1532,
        0x00b8,
        false,
    );
    let mut provider = devices::usb_device(&first).unwrap();
    provider.provider = ProviderKind::Openrazer;
    provider.id = "openrazer:SAME".to_owned();
    provider.identity.physical_path = None;
    let mut discovery = Discovery {
        devices: vec![provider],
        diagnostics: Vec::new(),
    };
    devices::merge_usb(&mut discovery, &[first, second]);
    assert_eq!(discovery.devices.len(), 3);
    assert_eq!(discovery.devices[0].identity.physical_path, None);
    let wired = usb(
        "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-4",
        None,
        0x046d,
        0xc094,
        false,
    );
    let mut provider = devices::usb_device(&wired).unwrap();
    provider.provider = ProviderKind::Ratbag;
    provider.identity.physical_path = Some(
        "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-4/1-4:1.0/0003:046D:C094.0009".to_owned(),
    );
    let mut discovery = Discovery {
        devices: vec![provider],
        diagnostics: Vec::new(),
    };
    devices::merge_usb(&mut discovery, &[wired]);
    assert_eq!(discovery.devices.len(), 1);
    assert_eq!(discovery.devices[0].provider, ProviderKind::Ratbag);
    assert!(discovery.devices[0]
        .identity
        .physical_path
        .as_ref()
        .unwrap()
        .ends_with("0003:046D:C094.0009"));
}

#[test]
fn anonymous_battery_cache_key_changes_on_usb_generation_and_not_enumeration_order() {
    let first = usb(
        "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-2",
        None,
        0x046d,
        0x0a66,
        true,
    );
    let mut changed = first.clone();
    changed.instance.push_str(":replacement");
    assert_ne!(
        headsetcontrol::usb_fingerprint(std::slice::from_ref(&first)),
        headsetcontrol::usb_fingerprint(&[changed])
    );
    let second = usb(
        "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-3",
        None,
        0x046d,
        0x0a66,
        true,
    );
    assert_eq!(
        headsetcontrol::usb_fingerprint(&[first.clone(), second.clone()]),
        headsetcontrol::usb_fingerprint(&[second, first])
    );
}

#[test]
fn empty_introspection_interface_is_valid_but_does_not_invent_capabilities() {
    let api = providers::parse_introspection("<node><interface name='empty'/></node>").unwrap();
    assert!(api.interfaces["empty"].methods.is_empty());
    assert!(!api.method("empty", "getCapabilities", "", "as"));
}

fn identity(serial: Option<&str>, path: Option<&str>, vendor: u16, product: u16) -> DeviceIdentity {
    DeviceIdentity {
        serial: serial.map(str::to_owned),
        physical_path: path.map(str::to_owned),
        vendor_id: Some(vendor),
        product_id: Some(product),
        ..DeviceIdentity::default()
    }
}

fn usb(path: &str, serial: Option<&str>, vendor: u16, product: u16, audio: bool) -> UsbMetadata {
    UsbMetadata {
        identity: identity(serial, Some(path), vendor, product),
        name: "Fixture USB device".to_owned(),
        is_audio: audio,
        bus_alias: devices::usb_bus_alias(path),
        instance: format!("{path}:1:7"),
    }
}

fn audio(
    name: &str,
    serial: Option<&str>,
    path: Option<&str>,
    vendor: u16,
    product: u16,
) -> AudioNode {
    AudioNode {
        id: 42,
        object_serial: Some(900),
        node_name: name.to_owned(),
        description: "Fixture audio".to_owned(),
        media_class: "Audio/Sink".to_owned(),
        channels: vec!["FL".to_owned(), "FR".to_owned()],
        volume: None,
        muted: None,
        is_virtual: false,
        device_serial: serial.map(str::to_owned),
        device_bus_path: path.map(str::to_owned),
        vendor_id: Some(vendor),
        product_id: Some(product),
        transport: Some("usb".to_owned()),
    }
}

#[test]
fn introspection_requires_exact_openrazer_signatures_without_invented_capabilities() {
    let api = providers::parse_introspection(include_str!("hardware_openrazer.xml")).unwrap();
    assert!(api.method(openrazer::MISC_INTERFACE, "getVidPid", "", "ai"));
    assert!(api.method(openrazer::DPI_INTERFACE, "setDPI", "qq", ""));
    assert!(!api.method(openrazer::DPI_INTERFACE, "setDPI", "ii", ""));
    assert!(api.method(openrazer::POWER_INTERFACE, "getBattery", "", "d"));
    assert!(!api.method("razer.device.misc", "getCapabilities", "", "as"));
    let manager =
        providers::parse_introspection(include_str!("hardware_openrazer_manager.xml")).unwrap();
    assert!(manager.method("razer.devices", "getDevices", "", "as"));
    assert_eq!(
        openrazer::parse_vid_pid(&[0x1532, 0x00b8]).unwrap(),
        (0x1532, 0x00b8)
    );
    assert!(openrazer::parse_vid_pid(&[0x1532, -1]).is_err());
    assert!(openrazer::parse_vid_pid(&[0x046d, 0x4074]).is_err());
    assert!(openrazer::parse_vid_pid(&[0x1532]).is_err());
}

#[test]
fn malformed_or_ambiguous_xml_is_rejected_without_external_entity_expansion() {
    for xml in ["<node><interface name='x'></node>", "<node/><node/>",
        "<!DOCTYPE node [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><node/>",
        "<node><interface name='x'><method name='m'/><method name='m'/></interface></node>",
        "<node><interface name='x'><property name='p' type='u' access='read'/><property name='p' type='u' access='read'/></interface></node>"] {
        assert!(providers::parse_introspection(xml).is_err(), "accepted {xml}");
    }
    assert!(
        providers::parse_introspection(&"x".repeat(providers::MAX_PROVIDER_BYTES + 1)).is_err()
    );
}

#[test]
fn viper_bounds_and_capability_failures_are_independent() {
    let api = providers::parse_introspection(include_str!("hardware_openrazer.xml")).unwrap();
    let current = Some(DpiValue { x: 800, y: 1200 });
    let dpi = openrazer::dpi_capability(&api, 0x00b8, Some(30000), None, current).unwrap();
    assert!(dpi.writable && dpi.separate_axes);
    assert!(dpi.values.contains(100) && dpi.values.contains(30000));
    assert!(!dpi.values.contains(99) && !dpi.values.contains(30001));
    let dpi_error = openrazer::dpi_capability(&api, 0x00b8, None, None, None).unwrap();
    let polling =
        openrazer::polling_capability(&api, 0x00b8, Some(vec![125, 500, 1000]), Some(500)).unwrap();
    assert!(!dpi_error.writable && polling.writable);
    assert!(!polling.values.contains(&250));
    assert!(
        !openrazer::polling_capability(&api, 0x00b8, Some(vec![125, 1000, 8000]), Some(1000))
            .unwrap()
            .writable
    );
    assert!(
        !openrazer::dpi_capability(&api, 0x9999, Some(20000), None, current)
            .unwrap()
            .writable
    );
    assert!(
        !openrazer::polling_capability(&api, 0x9999, None, Some(500))
            .unwrap()
            .writable
    );
    assert!(
        openrazer::polling_capability(&api, 0x9999, Some(vec![125, 250, 500]), Some(250))
            .unwrap()
            .writable
    );
    assert_eq!(
        openrazer::parse_dpi(&[800, 1200], false).unwrap(),
        DpiValue { x: 800, y: 1200 }
    );
    assert_eq!(
        openrazer::parse_dpi(&[800, 0], true).unwrap(),
        DpiValue { x: 800, y: 800 }
    );
    assert!(openrazer::parse_dpi(&[800, -1], false).is_err());
}

#[test]
fn razer_double_battery_is_bounded_and_viper_aa_is_never_charging() {
    for value in [-1.0, 100.1, f64::NAN, f64::INFINITY] {
        let battery = openrazer::battery(Some(value), Some(true), 0x00b8, 1000);
        assert_eq!(battery.percentage, None);
        assert_eq!(battery.charging, Some(false));
    }
    assert_eq!(
        openrazer::battery(Some(0.0), None, 0x00b8, 1000).percentage,
        Some(0.0)
    );
    assert_eq!(
        openrazer::battery(Some(64.5), None, 0x9999, 1000).charging,
        None
    );
}

#[test]
fn ratbag_contract_uses_properties_and_exact_api_two() {
    let manager =
        providers::parse_introspection(include_str!("hardware_ratbag_manager.xml")).unwrap();
    assert!(manager.property(ratbag::MANAGER_INTERFACE, "APIVersion", "i", false));
    assert!(manager.property(ratbag::MANAGER_INTERFACE, "Devices", "ao", false));
    for invalid in [-1, 0, 1, 3] {
        assert!(ratbag::validate_api_version(invalid).is_err());
    }
    ratbag::validate_api_version(2).unwrap();
    let device =
        providers::parse_introspection(include_str!("hardware_ratbag_device.xml")).unwrap();
    assert!(device.method(ratbag::DEVICE_INTERFACE, "Commit", "", "u"));
    assert!(device.signal(ratbag::DEVICE_INTERFACE, "Resync", ""));
    assert!(!device.method(ratbag::DEVICE_INTERFACE, "Refresh", "", ""));
    let profile =
        providers::parse_introspection(include_str!("hardware_ratbag_profile.xml")).unwrap();
    assert!(profile.property(ratbag::PROFILE_INTERFACE, "ReportRate", "u", true));
    assert!(profile.property(ratbag::PROFILE_INTERFACE, "ReportRates", "au", false));
    assert!(!profile.property(ratbag::PROFILE_INTERFACE, "ReportRates", "au", true));
    assert!(!profile.property(ratbag::PROFILE_INTERFACE, "Enabled", "b", false));
    let resolution =
        providers::parse_introspection(include_str!("hardware_ratbag_resolution.xml")).unwrap();
    assert!(resolution.property(ratbag::RESOLUTION_INTERFACE, "Resolution", "v", true));
    assert!(!resolution.method(ratbag::RESOLUTION_INTERFACE, "SetDPI", "u", ""));
    assert!(ratbag::parse_model("usb:1532:00b8:1").is_err());
    assert!(ratbag::parse_model("Logitech G305").is_err());
    assert!(ratbag::unsafe_profile_build(
        &ratbag::parse_model("usb:046d:c08b:1").unwrap()
    ));
    assert!(!ratbag::unsafe_profile_build(
        &ratbag::parse_model("usb:046d:4074:1").unwrap()
    ));
}

#[derive(Deserialize)]
struct Wire {
    property_signature: String,
    inner_signature: String,
    values: Vec<u32>,
}
impl Wire {
    fn decode(&self) -> ratbag::ResolutionValue {
        ratbag::ResolutionValue::from_wire(
            &self.property_signature,
            &self.inner_signature,
            &self.values,
        )
        .unwrap()
    }
}
#[derive(Deserialize)]
struct RatbagFixture {
    model: String,
    api_version: i32,
    profiles: Vec<ratbag::ProfileFlags>,
    resolutions: Vec<ratbag::ResolutionFlags>,
    dpi_values: Vec<u32>,
    report_rates: Vec<u32>,
    scalar: Wire,
    xy: Wire,
}
fn ratbag_fixture() -> RatbagFixture {
    serde_json::from_str(include_str!("hardware_ratbag_state.json")).unwrap()
}

#[test]
fn scalar_and_xy_preserve_inner_shape_and_nonuniform_discrete_values() {
    let fixture = ratbag_fixture();
    ratbag::validate_api_version(fixture.api_version).unwrap();
    assert_eq!(
        ratbag::parse_model(&fixture.model).unwrap().product_id,
        0x4074
    );
    let allowed = providers::ordered_values(fixture.dpi_values, 300).unwrap();
    let scalar = fixture.scalar.decode();
    let xy = fixture.xy.decode();
    assert_eq!(scalar.inner_signature(), "u");
    assert_eq!(xy.inner_signature(), "(uu)");
    assert_eq!(
        scalar.with_dpi(1200, 1200, &allowed, &[]).unwrap(),
        ratbag::ResolutionValue::Scalar(1200)
    );
    assert!(scalar.with_dpi(800, 1200, &allowed, &[]).is_err());
    assert!(scalar.with_dpi(1600, 1600, &allowed, &[]).is_err());
    assert_eq!(
        xy.with_dpi(1200, 3200, &allowed, &[1]).unwrap(),
        ratbag::ResolutionValue::Xy(1200, 3200)
    );
    assert!(xy.with_dpi(800, 1600, &allowed, &[1]).is_err());
    assert!(xy.with_dpi(800, 800, &allowed, &[]).is_err());
    assert!(ratbag::ResolutionValue::from_wire("u", "u", &[800]).is_err());
    assert!(ratbag::ResolutionValue::from_wire("v", "i", &[800]).is_err());
    assert!(ratbag::ResolutionValue::from_wire("v", "(uu)", &[800]).is_err());
    assert!(providers::ordered_values(vec![400, 400, 800], 300).is_err());
    assert!(providers::ordered_values(vec![0, 800], 300).is_err());
    assert_eq!(
        providers::ordered_values(vec![3200, 400, 1200, 800], 300).unwrap(),
        allowed
    );
    assert!(fixture.report_rates.contains(&250));
}

#[test]
fn active_flags_never_assume_zero_and_dirty_inactive_profile_blocks_writes() {
    let mut fixture = ratbag_fixture();
    assert_eq!(ratbag::active_profile(&fixture.profiles).unwrap(), 1);
    assert_eq!(ratbag::active_resolution(&fixture.resolutions).unwrap(), 1);
    ratbag::clean_profiles(&fixture.profiles).unwrap();
    fixture.profiles[0].is_dirty = Some(true);
    assert_eq!(
        ratbag::clean_profiles(&fixture.profiles).unwrap_err().code,
        ErrorCode::Busy
    );
    fixture.profiles[0].is_dirty = None;
    assert!(ratbag::clean_profiles(&fixture.profiles).is_err());
    fixture.profiles[0].is_active = Some(true);
    assert!(ratbag::active_profile(&fixture.profiles).is_err());
    fixture.resolutions[0].is_active = Some(true);
    assert!(ratbag::active_resolution(&fixture.resolutions).is_err());
    fixture.resolutions[0].is_active = Some(false);
    fixture.resolutions[1].disabled = Some(true);
    assert!(ratbag::active_resolution(&fixture.resolutions).is_err());
}

#[test]
fn queued_commit_and_cached_desired_value_are_never_success_without_completion_signal() {
    let mut evidence = ratbag::CommitEvidence::default();
    evidence.stage_readback(true, true, false);
    evidence.begin_commit();
    evidence.commit_reply(0);
    assert!(!evidence.can_confirm(true, true, true));
    evidence.dirty_signal(true, false);
    assert!(evidence.can_confirm(true, true, true));
    assert!(!evidence.can_confirm(false, true, true));
    assert!(!evidence.can_confirm(true, false, true));
    assert!(!evidence.can_confirm(true, true, false));
    evidence.resync = true;
    assert!(!evidence.can_confirm(true, true, true));
}

#[test]
fn ignored_staging_concurrent_dirty_and_resync_cannot_be_confirmed() {
    for (matches, dirty, other) in [
        (false, true, false),
        (true, false, false),
        (true, true, true),
    ] {
        let mut evidence = ratbag::CommitEvidence::default();
        evidence.stage_readback(matches, dirty, other);
        evidence.begin_commit();
        evidence.commit_reply(0);
        evidence.dirty_signal(true, false);
        assert!(!evidence.can_confirm(true, true, true));
    }
    let mut evidence = ratbag::CommitEvidence::default();
    evidence.stage_readback(true, true, false);
    evidence.dirty_signal(true, false);
    evidence.begin_commit();
    evidence.commit_reply(0);
    evidence.dirty_signal(true, false);
    assert!(!evidence.can_confirm(true, true, true));
    let mut evidence = ratbag::CommitEvidence::default();
    evidence.stage_readback(true, true, false);
    evidence.begin_commit();
    evidence.commit_reply(0);
    evidence.dirty_signal(false, true);
    evidence.dirty_signal(true, false);
    assert!(!evidence.can_confirm(true, true, true));
}

#[test]
fn upower_coarse_stale_unknown_and_zero_percent_remain_distinct() {
    let fixtures: Vec<upower::BatteryProperties> =
        serde_json::from_str(include_str!("hardware_upower.json")).unwrap();
    let samples: Vec<_> = fixtures.iter().map(|p| upower::battery(p, 1000)).collect();
    assert_eq!(samples[0].percentage, Some(64.2));
    assert_eq!(samples[0].source_updated_at, Some(990));
    assert_eq!(samples[0].observed_at, 1000);
    assert!(!samples[0].stale);
    assert_eq!(samples[1].percentage, None);
    assert_eq!(samples[1].level, Some(BatteryLevel::Normal));
    assert_eq!(samples[2].level, Some(BatteryLevel::Full));
    assert!(samples[2].stale);
    assert_eq!(samples[3].percentage, None);
    assert_eq!(samples[3].charging, None);
    assert_eq!(samples[3].source_updated_at, None);
    assert!(samples[3].stale);
    assert_eq!(samples[4].percentage, None);
    assert_eq!(samples[5].percentage, None);
    assert!(samples[5].stale);
    assert_eq!(samples[6].level, Some(BatteryLevel::Critical));
    assert_eq!(samples[7].percentage, Some(0.0));
    assert!(upower::compatible_type(5, DeviceKind::Mouse));
    assert!(!upower::compatible_type(2, DeviceKind::Mouse));
    assert!(!upower::compatible_type(17, DeviceKind::Mouse));
}

// Schema/export: Sapd/HeadsetControl 4.2.0 (91ee690a5e302a10a6f4f75a46be05846a38a3b0),
// cli/output/output{.cpp,_data.hpp} and tests/test_output_formats.cpp.
// IDs/capabilities: lib/devices/logitech_g{533,535,930}.hpp. Battery samples are synthetic.
fn headset_fixture() -> Value {
    serde_json::from_str(include_str!("hardware_headsetcontrol.json")).unwrap()
}

fn headset_report(fixture: &Value) -> headsetcontrol::Report {
    headsetcontrol::parse_json(&serde_json::to_vec(fixture).unwrap(), 1000).unwrap()
}

#[test]
fn headsetcontrol_json_validates_api_readability_status_and_partial_records() {
    let report =
        headsetcontrol::parse_json(include_bytes!("hardware_headsetcontrol.json"), 1000).unwrap();
    assert_eq!(report.api_version.as_deref(), Some("1.6"));
    assert_eq!(report.devices.len(), 3);
    assert_eq!(report.devices[0].vendor_id, 0x046d);
    assert_eq!(report.devices[0].product_id, 0x0a66);
    assert_eq!(report.devices[0].name, "Logitech G533");
    assert!(report.devices[0].readable_battery);
    assert_eq!(report.devices[0].battery.percentage, Some(64.0));
    assert_eq!(report.devices[0].battery.charging, Some(false));
    assert_eq!(report.devices[0].battery.observed_at, 1000);
    assert_eq!(
        report.devices[0].battery.source,
        ProviderKind::Headsetcontrol
    );
    assert_eq!(report.devices[1].vendor_id, 0x046d);
    assert_eq!(report.devices[1].product_id, 0x0ac4);
    assert!(report.devices[1].readable_battery);
    assert_eq!(report.devices[1].battery.percentage, Some(72.0));
    assert_eq!(report.devices[1].battery.charging, Some(true));
    for sample in &report.devices[2..] {
        assert_eq!(sample.battery.percentage, None);
        assert_eq!(sample.battery.charging, None);
    }
    for malformed in [
        &b"not JSON"[..],
        &b"[]"[..],
        &b"{}"[..],
        &b"{\"devices\":null}"[..],
        &b"{\"devices\":{}}"[..],
    ] {
        assert!(headsetcontrol::parse_json(malformed, 1000).is_err());
    }
    assert!(
        headsetcontrol::parse_json(&vec![b' '; providers::MAX_PROVIDER_BYTES + 1], 1000).is_err()
    );
    let mut oversized = headset_fixture();
    oversized["devices"] = json!(vec![
        oversized["devices"][0].clone();
        providers::MAX_DEVICES + 1
    ]);
    oversized["device_count"] = json!(providers::MAX_DEVICES + 1);
    assert!(headsetcontrol::parse_json(&serde_json::to_vec(&oversized).unwrap(), 1000).is_err());
}

#[test]
fn headsetcontrol_unknown_or_missing_api_never_exposes_battery() {
    for version in [
        json!("1.5"),
        json!("1.8"),
        json!("1.9"),
        json!("99.0"),
        json!(1.6),
        Value::Null,
    ] {
        let mut fixture = headset_fixture();
        fixture["api_version"] = version;
        let report = headset_report(&fixture);
        assert_eq!(report.devices.len(), 3);
        assert!(report
            .diagnostics
            .iter()
            .any(|d| d.id == "headsetcontrol-api"));
        assert!(report.devices.iter().all(|d| {
            !d.readable_battery && d.battery.percentage.is_none() && d.battery.charging.is_none()
        }));
    }
    let mut fixture = headset_fixture();
    fixture.as_object_mut().unwrap().remove("api_version");
    let report = headset_report(&fixture);
    assert_eq!(report.api_version, None);
    assert!(report
        .devices
        .iter()
        .all(|d| d.battery.percentage.is_none() && d.battery.charging.is_none()));
}

#[test]
fn headsetcontrol_missing_malformed_or_mismatched_count_is_fail_closed() {
    for count in [
        None,
        Some(Value::Null),
        Some(json!("3")),
        Some(json!(-1)),
        Some(json!(3.0)),
        Some(json!(2)),
        Some(json!(4)),
    ] {
        let mut fixture = headset_fixture();
        match count {
            Some(count) => fixture["device_count"] = count,
            None => {
                fixture.as_object_mut().unwrap().remove("device_count");
            }
        }
        let report = headset_report(&fixture);
        assert_eq!(report.devices.len(), 3);
        assert!(report
            .diagnostics
            .iter()
            .any(|d| d.id == "headsetcontrol-api"));
        assert!(report.devices.iter().all(|d| {
            !d.readable_battery && d.battery.percentage.is_none() && d.battery.charging.is_none()
        }));
    }
}

#[test]
fn headsetcontrol_readability_uses_only_the_verified_battery_capability() {
    let mut fixture = headset_fixture();
    assert!(fixture["devices"][0].get("readable_capabilities").is_none());
    fixture["devices"][0]["readable_capabilities"] = json!([]);
    assert_eq!(
        headset_report(&fixture).devices[0].battery.percentage,
        Some(64.0)
    );
    for caps in [
        None,
        Some(Value::Null),
        Some(json!([])),
        Some(json!(["battery"])),
        Some(json!(["CAP_SIDETONE"])),
        Some(json!(["CAP_BATTERY_STATUS", 1])),
        Some(json!(vec!["CAP_BATTERY_STATUS"; 129])),
    ] {
        let mut fixture = headset_fixture();
        fixture["devices"][0]["readable_capabilities"] = json!(["CAP_BATTERY_STATUS"]);
        match caps {
            Some(caps) => fixture["devices"][0]["capabilities"] = caps,
            None => {
                fixture["devices"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("capabilities");
            }
        }
        let report = headset_report(&fixture);
        assert!(!report.devices[0].readable_battery);
        assert_eq!(report.devices[0].battery.percentage, None);
        assert_eq!(report.devices[0].battery.charging, None);
    }
}

#[test]
fn headsetcontrol_partial_failed_malformed_and_unknown_batteries_are_not_values() {
    let mut records = Vec::new();
    for status in ["partial", "failure", "unknown", "invented"] {
        let mut device = headset_fixture()["devices"][0].clone();
        device["status"] = json!(status);
        records.push(device);
    }
    for errors in [
        json!({"battery": "HID timeout"}),
        json!({"battery": null}),
        json!([]),
        Value::Null,
    ] {
        let mut device = headset_fixture()["devices"][0].clone();
        device["errors"] = errors;
        records.push(device);
    }
    for status in [
        "BATTERY_UNAVAILABLE",
        "BATTERY_ERROR",
        "BATTERY_TIMEOUT",
        "UNKNOWN",
        "invented",
    ] {
        let mut device = headset_fixture()["devices"][0].clone();
        device["battery"]["status"] = json!(status);
        records.push(device);
    }
    for status in ["BATTERY_AVAILABLE", "BATTERY_CHARGING"] {
        for level in [
            None,
            Some(Value::Null),
            Some(json!("64")),
            Some(json!(64.5)),
            Some(json!(-2)),
            Some(json!(101)),
        ] {
            let mut device = headset_fixture()["devices"][0].clone();
            device["battery"]["status"] = json!(status);
            match level {
                Some(level) => device["battery"]["level"] = level,
                None => {
                    device["battery"].as_object_mut().unwrap().remove("level");
                }
            }
            records.push(device);
        }
    }
    for battery in [None, Some(Value::Null), Some(json!({})), Some(json!(64))] {
        let mut device = headset_fixture()["devices"][0].clone();
        match battery {
            Some(battery) => device["battery"] = battery,
            None => {
                device.as_object_mut().unwrap().remove("battery");
            }
        }
        records.push(device);
    }
    let mut invalid_available = headset_fixture()["devices"][0].clone();
    invalid_available["battery"]["level"] = json!(-1);
    records.push(invalid_available);
    for device in records {
        let mut fixture = headset_fixture();
        fixture["devices"][0] = device;
        let report = headset_report(&fixture);
        assert_eq!(report.devices[0].battery.percentage, None, "{fixture}");
        assert_eq!(report.devices[0].battery.charging, None, "{fixture}");
        assert_eq!(report.devices[1].battery.percentage, Some(72.0));
    }
}

#[test]
fn headsetcontrol_charging_unknown_level_and_percent_boundaries_are_not_invented() {
    let mut fixture = headset_fixture();
    fixture["devices"][1]["battery"]["level"] = json!(-1);
    let report = headset_report(&fixture);
    assert_eq!(report.devices[1].battery.percentage, None);
    assert_eq!(report.devices[1].battery.level, None);
    assert_eq!(report.devices[1].battery.charging, Some(true));
    for level in [0, 100] {
        fixture["devices"][0]["battery"]["level"] = json!(level);
        assert_eq!(
            headset_report(&fixture).devices[0].battery.percentage,
            Some(level as f64)
        );
    }
}

#[test]
fn headsetcontrol_probe_checks_the_same_contract_without_hardware() {
    use peripheral_hub_lib::process::ProcessOutput;
    let mut output = ProcessOutput {
        stdout: include_bytes!("hardware_headsetcontrol.json").to_vec(),
        stderr: String::new(),
    };
    assert!(headsetcontrol::conforms(&output));
    output.stderr = " \n".to_owned();
    assert!(headsetcontrol::conforms(&output));
    output.stderr = "HID error".to_owned();
    assert!(!headsetcontrol::conforms(&output));
    output.stderr.clear();
    let mut invalid = Vec::new();
    for version in [
        json!("1.8"),
        json!("1.9"),
        json!("99.0"),
        json!(1.6),
        Value::Null,
    ] {
        let mut fixture = headset_fixture();
        fixture["api_version"] = version;
        invalid.push(fixture);
    }
    for field in ["name", "api_version", "device_count"] {
        let mut fixture = headset_fixture();
        fixture.as_object_mut().unwrap().remove(field);
        invalid.push(fixture);
    }
    let mut fixture = headset_fixture();
    fixture["devices"][0]["capabilities"] = json!([1]);
    invalid.push(fixture);
    for fixture in invalid {
        output.stdout = serde_json::to_vec(&fixture).unwrap();
        assert!(!headsetcontrol::conforms(&output), "{fixture}");
    }
    output.stdout = b"not JSON".to_vec();
    assert!(!headsetcontrol::conforms(&output));
    let mut empty = headset_fixture();
    empty["devices"] = json!([]);
    empty["device_count"] = json!(0);
    output.stdout = serde_json::to_vec(&empty).unwrap();
    assert!(headsetcontrol::conforms(&output));
}

#[test]
fn headsetcontrol_preserves_vendor_ids_without_undocumented_razer_battery_support() {
    let mut fixture = headset_fixture();
    fixture["devices"] = json!([
        fixture["devices"][0],
        {"status": "success", "device": "Razer USB metadata", "id_vendor": "0x1532", "id_product": "0x057d", "capabilities": [], "capabilities_str": []},
        {"status": "success", "device": "HeadsetControl Test device", "id_vendor": "0xf00b", "id_product": "0xa00c", "capabilities": ["CAP_BATTERY_STATUS"], "capabilities_str": ["battery"], "battery": {"status": "BATTERY_AVAILABLE", "level": 42}}
    ]);
    let report = headset_report(&fixture);
    assert_eq!(report.devices.len(), 2);
    assert_eq!(
        (report.devices[0].vendor_id, report.devices[0].product_id),
        (0x046d, 0x0a66)
    );
    assert_eq!(
        (report.devices[1].vendor_id, report.devices[1].product_id),
        (0x1532, 0x057d)
    );
    assert!(!report.devices[1].readable_battery);
    assert_eq!(report.devices[1].battery.percentage, None);
    assert_eq!(report.devices[1].battery.charging, None);
}

#[test]
fn headsetcontrol_duplicate_json_and_unidentified_records_do_not_associate_hardware() {
    let usb = vec![usb(
        "/sys/devices/usb1/1-2",
        Some("HEADSET_A"),
        0x046d,
        0x0a66,
        true,
    )];
    let mut fixture = headset_fixture();
    let mut duplicate = fixture["devices"][0].clone();
    duplicate["battery"]["level"] = json!(42);
    fixture["devices"].as_array_mut().unwrap().push(duplicate);
    fixture["device_count"] = json!(4);
    let report = headset_report(&fixture);
    assert_eq!(report.devices[0].battery.percentage, Some(64.0));
    assert_eq!(report.devices[3].battery.percentage, Some(42.0));
    let mut discovery = Discovery::default();
    devices::merge_usb(&mut discovery, &usb);
    devices::associate_headset_batteries(&mut discovery, &usb, &report);
    assert!(discovery.devices[0].values.battery.is_none());
    assert!(discovery
        .diagnostics
        .iter()
        .any(|d| d.id == "headsetcontrol-ambiguous"));
    for record in [
        Value::Null,
        json!({}),
        json!({"id_vendor": "0x046d"}),
        json!({"id_vendor": "0x046d", "id_product": "0x10000"}),
    ] {
        let mut fixture = headset_fixture();
        fixture["devices"].as_array_mut().unwrap().push(record);
        fixture["device_count"] = json!(4);
        let report = headset_report(&fixture);
        assert!(report
            .devices
            .iter()
            .all(|d| d.battery.percentage.is_none() && d.battery.charging.is_none()));
    }
}

#[test]
fn headsetcontrol_fingerprint_only_tracks_supported_vendor_audio_and_generation() {
    let logitech = usb(
        "/sys/devices/usb1/1-2",
        Some("LOGITECH_A"),
        0x046d,
        0x0a66,
        true,
    );
    let razer = usb(
        "/sys/devices/usb1/1-3",
        Some("RAZER_A"),
        0x1532,
        0x057d,
        true,
    );
    let key = headsetcontrol::usb_fingerprint(&[logitech.clone(), razer.clone()]);
    assert_ne!(
        headsetcontrol::usb_fingerprint(std::slice::from_ref(&razer)),
        "[]"
    );
    assert_eq!(
        key,
        headsetcontrol::usb_fingerprint(&[razer.clone(), logitech.clone()])
    );
    let mut replacement = razer.clone();
    replacement.instance.push_str(":replacement");
    assert_ne!(
        key,
        headsetcontrol::usb_fingerprint(&[logitech.clone(), replacement])
    );
    let mut replacement = razer.clone();
    replacement.identity.serial = Some("RAZER_B".to_owned());
    assert_ne!(
        key,
        headsetcontrol::usb_fingerprint(&[logitech.clone(), replacement])
    );
    assert_ne!(
        key,
        headsetcontrol::usb_fingerprint(std::slice::from_ref(&logitech))
    );
    let mut incomplete = logitech.clone();
    incomplete.identity.product_id = None;
    let ignored = [
        usb("/sys/devices/usb1/1-4", None, 0xf00b, 0xa00c, true),
        usb("/sys/devices/usb1/1-5", None, 0x046d, 0xc094, false),
        usb("/sys/devices/usb1/1-6", None, 0x1532, 0x00b8, false),
        incomplete,
    ];
    assert_eq!(headsetcontrol::usb_fingerprint(&ignored), "[]");
    let mut with_irrelevant = vec![logitech, razer];
    with_irrelevant.extend(ignored);
    assert_eq!(key, headsetcontrol::usb_fingerprint(&with_irrelevant));
}

#[test]
fn identity_never_uses_name_or_vid_pid_and_duplicate_serial_is_ambiguous() {
    let a = identity(None, None, 0x046d, 0x0aff);
    assert!(!providers::identities_match(&a, &a));
    let a = identity(Some("SERIAL_A"), None, 0x046d, 0x0aff);
    assert_eq!(
        providers::unique_identity_match(&a, &[a.clone(), a.clone()]),
        None
    );
    let first = identity(
        Some("SERIAL_A"),
        Some("/sys/devices/pci0000:00/0000:00:14.0/usb1/1-2"),
        0x046d,
        0x0aff,
    );
    let second = identity(
        Some("SERIAL_A"),
        Some("/sys/devices/pci0000:00/0000:00:14.0/usb1/1-3"),
        0x046d,
        0x0aff,
    );
    assert!(!providers::identities_match(&first, &second));
    assert_eq!(
        providers::unique_identity_match(&first, &[first.clone(), second]),
        Some(0)
    );
    let changed = identity(
        Some("SERIAL_B"),
        first.physical_path.as_deref(),
        0x046d,
        0x0aff,
    );
    assert!(!providers::identities_match(&first, &changed));
}

#[test]
fn duplicate_headsets_do_not_receive_arbitrary_battery_or_vendor_audio_mapping() {
    let usb = vec![
        usb(
            "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-2",
            None,
            0x046d,
            0x0a66,
            true,
        ),
        usb(
            "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-3",
            None,
            0x046d,
            0x0a66,
            true,
        ),
    ];
    let mut discovery = Discovery::default();
    devices::merge_usb(&mut discovery, &usb);
    let report =
        headsetcontrol::parse_json(include_bytes!("hardware_headsetcontrol.json"), 1000).unwrap();
    assert_eq!(report.devices[0].battery.percentage, Some(64.0));
    devices::associate_headset_batteries(&mut discovery, &usb, &report);
    assert!(discovery.devices.iter().all(|d| d.values.battery.is_none()));
    assert!(discovery
        .diagnostics
        .iter()
        .any(|d| d.id == "headsetcontrol-ambiguous"));
    devices::associate_audio(
        &mut discovery,
        &[
            audio("alsa_output.a", None, None, 0x046d, 0x0a66),
            audio("alsa_output.b", None, None, 0x046d, 0x0a66),
        ],
        &usb,
    );
    assert_eq!(
        discovery
            .devices
            .iter()
            .filter(|d| d.brand == Brand::Generic)
            .count(),
        2
    );
}

#[test]
fn verified_physical_audio_mapping_avoids_generic_duplicate_and_single_usb_battery_associates() {
    let usb = vec![usb(
        "/sys/devices/pci0000:00/0000:00:14.0/usb1/1-2",
        Some("HEADSET_A"),
        0x046d,
        0x0a66,
        true,
    )];
    let mut discovery = Discovery::default();
    devices::merge_usb(&mut discovery, &usb);
    let id = discovery.devices[0].id.clone();
    let report =
        headsetcontrol::parse_json(include_bytes!("hardware_headsetcontrol.json"), 1000).unwrap();
    devices::associate_headset_batteries(&mut discovery, &usb, &report);
    devices::associate_audio(
        &mut discovery,
        &[audio(
            "alsa_output.a",
            None,
            Some("pci-0000:00:14.0-usb-0:2:1.0"),
            0x046d,
            0x0a66,
        )],
        &usb,
    );
    assert_eq!(discovery.devices.len(), 1);
    assert_eq!(discovery.devices[0].id, id);
    assert_eq!(discovery.devices[0].output_node_names, ["alsa_output.a"]);
    assert_eq!(
        discovery.devices[0]
            .values
            .battery
            .as_ref()
            .unwrap()
            .percentage,
        Some(64.0)
    );
    assert!(devices::verified_usb_path("pci-0000:00:14.0-usb-0:20:1.0", &usb).is_none());
    assert!(devices::usb_physical_path("/sys/devices/../usb1/1-2").is_none());
}

#[test]
fn headsetcontrol_audio_identity_requires_matching_vendor_product_and_physical_metadata() {
    let usb = vec![usb("/sys/devices/usb1/1-2", None, 0x046d, 0x0a66, true)];
    let mut discovery = Discovery::default();
    devices::merge_usb(&mut discovery, &usb);
    devices::associate_headset_batteries(&mut discovery, &usb, &headset_report(&headset_fixture()));
    devices::associate_audio(
        &mut discovery,
        &[
            audio(
                "alsa_output.vendor_mismatch",
                None,
                Some("/sys/devices/usb1/1-2"),
                0x1532,
                0x057d,
            ),
            audio(
                "alsa_output.product_mismatch",
                None,
                Some("/sys/devices/usb1/1-2"),
                0x046d,
                0x0ac4,
            ),
            audio("alsa_output.no_identity", None, None, 0x046d, 0x0a66),
        ],
        &usb,
    );
    assert_eq!(discovery.devices.len(), 4);
    assert_eq!(
        discovery.devices[0]
            .values
            .battery
            .as_ref()
            .unwrap()
            .percentage,
        Some(64.0)
    );
    assert!(discovery.devices[0].output_node_names.is_empty());
    assert!(discovery.devices[1..]
        .iter()
        .all(|d| d.brand == Brand::Generic && d.values.battery.is_none()));
}

#[test]
fn receivers_and_proposed_blackshark_ids_are_metadata_only_not_mouse_write_owners() {
    let receiver =
        devices::usb_device(&usb("/sys/devices/usb1/1-2", None, 0x046d, 0xc547, false)).unwrap();
    assert_eq!(receiver.kind, DeviceKind::Receiver);
    assert!(receiver.capabilities.dpi.is_none());
    let hyper =
        devices::usb_device(&usb("/sys/devices/usb1/1-3", None, 0x1532, 0x00b3, false)).unwrap();
    assert_eq!(hyper.kind, DeviceKind::Receiver);
    let blackshark =
        devices::usb_device(&usb("/sys/devices/usb1/1-4", None, 0x1532, 0x057d, true)).unwrap();
    assert!(blackshark.values.battery.is_none());
    assert!(blackshark.capabilities.dpi.is_none());
    assert!(blackshark.limitations.iter().any(|l| l.contains("#2899")));
    assert!(blackshark.limitations.iter().any(|l| l.contains("dongle")));
}

#[cfg(not(target_os = "linux"))]
#[tokio::test]
async fn unsupported_platform_never_falls_back_to_simulated_hardware() {
    let discovery = devices::discover(&[], true).await;
    assert!(discovery.devices.is_empty());
    assert_eq!(
        discovery.diagnostics[0].code,
        AvailabilityCode::UnsupportedPlatform
    );
    let device = devices::usb_device(&usb(
        "/sys/devices/usb1/1-2",
        Some("A"),
        0x1532,
        0x00b8,
        false,
    ))
    .unwrap();
    assert_eq!(
        devices::set_dpi(&device, 800, 800).await.unwrap_err().code,
        ErrorCode::UnsupportedPlatform
    );
    assert_eq!(
        devices::set_polling_rate(&device, 500)
            .await
            .unwrap_err()
            .code,
        ErrorCode::UnsupportedPlatform
    );
}

#[cfg(target_os = "linux")]
#[test]
fn properties_set_serialization_keeps_outer_property_variant_and_scalar_xy() {
    use zbus::zvariant::{serialized::Context, to_bytes, OwnedValue, LE};
    for resolution in [
        ratbag::ResolutionValue::Scalar(800),
        ratbag::ResolutionValue::Xy(800, 1200),
    ] {
        let body = (
            ratbag::RESOLUTION_INTERFACE,
            "Resolution",
            ratbag::resolution_set_value(resolution),
        );
        let bytes = to_bytes(Context::new_dbus(LE, 0), &body).unwrap();
        let ((interface, name, value), _): ((String, String, OwnedValue), _) =
            bytes.deserialize().unwrap();
        assert_eq!(interface, ratbag::RESOLUTION_INTERFACE);
        assert_eq!(name, "Resolution");
        assert_eq!(ratbag::decode_resolution(&value).unwrap(), resolution);
    }
    assert!(ratbag::decode_resolution(&OwnedValue::from(800u32)).is_err());
}
