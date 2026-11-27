// JSON replies for the sidecar protocol (see PROTOCOL.md).
// AOT-safe: every reply is one concrete DTO serialised through the
// source-generated context below. Requests are parsed with
// JsonDocument (trim-safe DOM, no reflection). No anonymous types,
// no Assembly.GetName().Version, no reflection anywhere — that is
// what the spike's IL3050/IL2026 warnings came from.
using System.Text.Json;
using System.Text.Json.Serialization;

namespace RshHmSidecar;

/// <summary>
/// One reply shape for every command. Fields that do not apply to a
/// command are null and omitted from the JSON. The Rust side parses
/// the same shape with all-optional fields.
/// </summary>
internal sealed record SidecarReply
{
    [JsonPropertyName("ok")]
    public bool Ok { get; init; }

    [JsonPropertyName("code")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? Code { get; init; }

    [JsonPropertyName("error")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? Error { get; init; }

    // Hello / identity.
    [JsonPropertyName("hello")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? Hello { get; init; }

    [JsonPropertyName("sidecar")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? Sidecar { get; init; }

    [JsonPropertyName("sdk")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? Sdk { get; init; }

    [JsonPropertyName("protocol")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public int? Protocol { get; init; }

    [JsonPropertyName("pid")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public int? Pid { get; init; }

    [JsonPropertyName("token")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? Token { get; init; }

    [JsonPropertyName("transport")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? Transport { get; init; }

    [JsonPropertyName("os")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? Os { get; init; }

    [JsonPropertyName("admin")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public bool? Admin { get; init; }

    // Driver / SDK state.
    [JsonPropertyName("driver_installed")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public bool? DriverInstalled { get; init; }

    [JsonPropertyName("driver")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? Driver { get; init; }

    [JsonPropertyName("dll_sha256")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? DllSha256 { get; init; }

    [JsonPropertyName("dll_sha256_ok")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public bool? DllSha256Ok { get; init; }

    // Install-driver.
    [JsonPropertyName("was_installed")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public bool? WasInstalled { get; init; }

    [JsonPropertyName("now_installed")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public bool? NowInstalled { get; init; }

    [JsonPropertyName("outcome")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? Outcome { get; init; }

    // Create / controller.
    [JsonPropertyName("created")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public bool? Created { get; init; }

    [JsonPropertyName("profile")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? Profile { get; init; }

    [JsonPropertyName("transport_index")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public int? TransportIndex { get; init; }

    [JsonPropertyName("create_ms")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public double? CreateMs { get; init; }

    [JsonPropertyName("capabilities")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public CapabilitiesReply? Capabilities { get; init; }

    // Profile description.
    [JsonPropertyName("vid")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public int? Vid { get; init; }

    [JsonPropertyName("pid_value")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public int? PidValue { get; init; }

    [JsonPropertyName("vid_pid_provisional")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public bool? VidPidProvisional { get; init; }

    [JsonPropertyName("descriptor_len")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public int? DescriptorLen { get; init; }

    [JsonPropertyName("descriptor_hex")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? DescriptorHex { get; init; }

    [JsonPropertyName("input_report_size")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public int? InputReportSize { get; init; }

    [JsonPropertyName("input_report_size_legacy_assumption")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public int? InputReportSizeLegacyAssumption { get; init; }

    [JsonPropertyName("button_count")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public int? ButtonCount { get; init; }

    [JsonPropertyName("axis_count")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public int? AxisCount { get; init; }

    [JsonPropertyName("has_hat")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public bool? HasHat { get; init; }

    [JsonPropertyName("backend")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? Backend { get; init; }

    // Submit / ping.
    [JsonPropertyName("pong")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public bool? Pong { get; init; }

    [JsonPropertyName("t_us")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public double? TUs { get; init; }

    [JsonPropertyName("submit_us")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public double? SubmitUs { get; init; }

    [JsonPropertyName("on_submit_latency_us")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public long? OnSubmitLatencyUs { get; init; }

    [JsonPropertyName("submit_count")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public long? SubmitCount { get; init; }

    // G29 spoof (v2.0.16): output/FFB packets received on the G29
    // controller and discarded (no motor) — a counter only, never
    // payload data. Null when no G29 controller was ever created.
    [JsonPropertyName("output_received_count")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public long? OutputReceivedCount { get; init; }

    // Dispose / exit.
    [JsonPropertyName("disposed")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public bool? Disposed { get; init; }

    [JsonPropertyName("bye")]
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public bool? Bye { get; init; }
}

/// <summary>Capability flags for the created controller's profile.</summary>
internal sealed record CapabilitiesReply
{
    [JsonPropertyName("axes")]
    public int Axes { get; init; }

    [JsonPropertyName("buttons")]
    public int Buttons { get; init; }

    [JsonPropertyName("has_hat")]
    public bool HasHat { get; init; }

    [JsonPropertyName("input_report_size")]
    public int InputReportSize { get; init; }

    [JsonPropertyName("submit_state_max_button")]
    public int SubmitStateMaxButton { get; init; }

    [JsonPropertyName("submit_state_button_mask")]
    public uint SubmitStateButtonMask { get; init; }
}

/// <summary>
/// Source-generated JSON context. The whole protocol is serialised
/// through this — no reflection, no anonymous types, so the NativeAOT
/// publish stays free of IL3050/IL2026.
/// </summary>
[JsonSerializable(typeof(SidecarReply))]
[JsonSerializable(typeof(CapabilitiesReply))]
internal sealed partial class SidecarJsonContext : JsonSerializerContext;
