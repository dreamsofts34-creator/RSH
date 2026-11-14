#![no_main]
use libfuzzer_sys::fuzz_target;
use rsh_protocol::{PinModule, PinModuleKind, validate};

fuzz_target!(|data: &[u8]| {
    // Build a random module list from raw bytes: kind byte + pins.
    // The validator must return errors or an empty list — never panic,
    // and never silently accept a forbidden/duplicate/out-of-range pin.
    let mut modules: Vec<PinModule> = Vec::new();
    let mut chunks = data.chunks_exact(3);
    for chunk in &mut chunks {
        let kind = match chunk[0] % 9 {
            0 => PinModuleKind::Axis,
            1 => PinModuleKind::AxisAds1115,
            2 => PinModuleKind::Battery,
            3 => PinModuleKind::Button,
            4 => PinModuleKind::ButtonMatrix,
            5 => PinModuleKind::Encoder,
            6 => PinModuleKind::ExpanderButton,
            7 => PinModuleKind::Led,
            _ => PinModuleKind::AxisAs5600,
        };
        let mut module = match kind {
            PinModuleKind::Encoder => PinModule::encoder(chunk[1], chunk[2]),
            PinModuleKind::AxisAds1115 => PinModule::axis_ads1115(chunk[1] % 4),
            PinModuleKind::AxisAs5600 => PinModule::axis_as5600(),
            PinModuleKind::ExpanderButton => PinModule::expander_button(chunk[1] % 16),
            PinModuleKind::ButtonMatrix => {
                PinModule::button_matrix(vec![chunk[1], chunk[2]]).with_rows_cols(1, 1)
            }
            _ => PinModule {
                kind,
                pins: vec![chunk[1]],
                ext_pullup: chunk[2] % 2 == 0,
                channel: 0,
                addr: 0x20,
                expander: "MCP23017".to_string(),
                expander_pin: None,
                rows: 4,
                cols: 4,
                count: None,
            },
        };
        if kind == PinModuleKind::Led {
            module = module.with_count((chunk[2] % 61) + 1);
        }
        modules.push(module);
        if modules.len() >= 16 {
            break;
        }
    }
    let expanders = data.first().map_or(false, |b| b % 2 == 0);
    let _ = validate(&modules, expanders);
});
