//! Protocol integration tests: wire encoding, hex reports, reply
//! parsing — the JSON contract the Rust side and the C# sidecar share
//! (see `tools/rsh-hm-sidecar/PROTOCOL.md`).

use rsh_output_hidmaestro::protocol::{
    AxesJson, Command, bytes_to_hex, encode_command, parse_reply, reply_to_error,
};
use rsh_output_hidmaestro::{GamepadFrame, HidMaestroError};

#[test]
fn every_command_encodes_with_the_right_cmd_name() {
    let cases: Vec<(Command, &str)> = vec![
        (Command::Ping, "ping"),
        (Command::Version, "version"),
        (Command::DescribeProfile, "describe-profile"),
        (Command::InstallDriver, "install-driver"),
        (
            Command::Create {
                profile: "rsh-wheel-custom".to_string(),
            },
            "create",
        ),
        (
            Command::Submit {
                axes: AxesJson {
                    x: 0.5,
                    y: 0.0,
                    z: 0.0,
                    rx: 0.0,
                    slider: 0.0,
                },
                buttons: 0,
            },
            "submit",
        ),
        (
            Command::SubmitRaw {
                report: "00".to_string(),
            },
            "submit-raw",
        ),
        (Command::Neutral, "neutral"),
        (Command::Dispose, "dispose"),
        (Command::Exit, "exit"),
    ];
    for (cmd, name) in cases {
        let line = encode_command(&cmd).unwrap();
        assert!(!line.contains('\n'), "one JSON line: {line}");
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["cmd"], name);
    }
}

#[test]
fn submit_wire_line_is_compact() {
    let line = encode_command(&Command::Submit {
        axes: AxesJson {
            x: 0.5,
            y: 0.0,
            z: 0.0,
            rx: 0.0,
            slider: 0.0,
        },
        buttons: 0,
    })
    .unwrap();
    // The spike measured 84 bytes on the wire for this frame.
    assert!(
        line.len() < 160,
        "submit line is {len} bytes: {line}",
        len = line.len()
    );
}

#[test]
fn bytes_to_hex_cases() {
    assert_eq!(bytes_to_hex(&[]), "");
    assert_eq!(bytes_to_hex(&[0x00, 0xff, 0xab, 0x08]), "00ffab08");
    assert_eq!(bytes_to_hex(&[0u8; 15]).len(), 30);
}

#[test]
fn axes_json_round_trip() {
    let frame = GamepadFrame {
        x: 0.75,
        y: 0.25,
        z: 1.0,
        rx: 0.0,
        slider: 0.5,
        buttons: 7,
    };
    let axes = AxesJson::from(frame);
    let json = serde_json::to_string(&axes).unwrap();
    let back: AxesJson = serde_json::from_str(&json).unwrap();
    assert_eq!(back, axes);
}

#[test]
fn parse_full_create_reply_with_caps() {
    let reply = parse_reply(
        r#"{"ok":true,"created":true,"profile":"rsh-wheel-custom","transport_index":0,"driver":"2.1.0","capabilities":{"has_axis_x":true,"has_axis_y":true,"has_axis_z":true,"has_axis_rx":true,"has_axis_slider":true,"button_count":32,"has_hat":true}}"#,
    )
    .unwrap();
    assert!(reply.ok);
    assert_eq!(reply.transport_index, Some(0));
    let caps = reply.capabilities.unwrap();
    assert!(caps.has_axis_x);
    assert_eq!(caps.button_count, 32);
    assert!(caps.has_hat);
}

#[test]
fn parse_full_hello_reply() {
    let reply = parse_reply(
        r#"{"ok":true,"hello":"rsh-hm-sidecar","sidecar":"rsh-hm-sidecar","sdk":"2.1.0","protocol":1,"pid":1234,"admin":true,"driver_installed":false,"dll_sha256":"f13e32591404db0f418af9265b8e240f71257297cfae5a41dcd696fd87e9b4d5","dll_sha256_ok":true,"transport":"stdio"}"#,
    )
    .unwrap();
    assert_eq!(reply.hello.as_deref(), Some("rsh-hm-sidecar"));
    assert_eq!(reply.protocol, Some(1));
    assert_eq!(reply.pid, Some(1234));
    assert_eq!(reply.admin, Some(true));
}

#[test]
fn parse_describe_profile_reply() {
    let reply = parse_reply(
        r#"{"ok":true,"profile":"rsh-wheel-custom","vid":4617,"pid_value":21075,"vid_pid_provisional":true,"descriptor_len":124,"descriptor_hex":"05010904","input_report_size":15,"input_report_size_legacy_assumption":26}"#,
    )
    .unwrap();
    assert_eq!(reply.vid, Some(0x1209));
    assert_eq!(reply.pid_value, Some(0x5253));
    assert_eq!(reply.input_report_size, Some(15));
    assert_eq!(reply.input_report_size_legacy_assumption, Some(26));
}

#[test]
fn reply_to_error_unknown_command() {
    let reply =
        parse_reply(r#"{"ok":false,"code":"unknown-command","error":"unknown cmd 'bogus'"}"#)
            .unwrap();
    assert!(matches!(
        reply_to_error(&reply),
        Err(HidMaestroError::BadReply(_))
    ));
}
