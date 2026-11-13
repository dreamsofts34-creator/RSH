using HIDMaestro;

namespace RSH.Output.HIDMaestro;

/// <summary>
/// HIDMaestro SDK ka darwaza — test seam (RSH.App ka ControlTransportOverride
/// jaisa pattern): asal implementation <see cref="HidMaestroSdkAdapter"/> hai,
/// tests fake dete hain. Is se poora <see cref="HidMaestroOutput"/> lifecycle
/// baghair driver ke test hota hai.
/// </summary>
public interface IHidMaestroSdk : IDisposable
{
    string ProfileId { get; }
    string? SdkVersion { get; }
    string? DriverVersionText { get; }

    /// <summary>Connected device ki salahiyatein (create se pehle Unknown).</summary>
    HidMaestroCapabilities Capabilities { get; }

    /// <summary>
    /// Driver install (zaroorat par, admin) + profile load + device create.
    /// Nakami par error mein wajah (insani zuban mein) — kabhi throw nahi.
    /// </summary>
    bool TryCreateDevice(out string error);

    void SubmitState(in HMGamepadState state);
    void SubmitRawReport(ReadOnlySpan<byte> report);

    /// <summary>Device hatao + context band karo (idempotent).</summary>
    void DestroyDevice();
}
