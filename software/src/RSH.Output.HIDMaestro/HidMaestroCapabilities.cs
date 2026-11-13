using HIDMaestro;

namespace RSH.Output.HIDMaestro;

/// <summary>
/// HIDMaestro device ki salahiyatein — <see cref="HMProfile"/> se parhi
/// jati hain (TryConnect ke baad sahi hoti hain). VJoyCapabilities jaisa
/// pattern (P12): validation configured modules ke khilaf hoti hai.
/// <see cref="AvailableAxes"/> profile ke descriptor se aata hai — jo
/// axis profile mein nahi, wo SDK khamoshi se ignore karta hai, is liye
/// mapping usay pehle se chhant ti hai (HidMaestroMapping).
/// </summary>
public readonly record struct HidMaestroCapabilities(
    bool HasAxisX,
    bool HasAxisY,
    bool HasAxisZ,
    bool HasAxisRx,
    bool HasAxisSlider,
    int ButtonCount,
    bool HasHat = false)
{
    /// <summary>Buttons ki hadd — vJoy jaisi (0..128); raw-report path isi hadd mein rehta hai.</summary>
    public const int MaxButtons = 128;

    /// <summary>
    /// Caps abhi parhe nahi gaye (connect nahi hua) — "unknown" ka matlab
    /// purana behaviour: sab kuch likho (koi gating nahi). VJoyCapabilities
    /// .Unknown jaisa — ButtonCount 128 = poora device farz karo.
    /// </summary>
    public static HidMaestroCapabilities Unknown => new(true, true, true, true, true, 128, HasHat: false);

    /// <summary>Profile se caps banao — HMProfile kabhi null nahi hota (adapter pehle check karta hai).</summary>
    public static HidMaestroCapabilities FromProfile(HMProfile profile)
    {
        ArgumentNullException.ThrowIfNull(profile);
        var axes = profile.AvailableAxes;
        bool Has(HMAxis axis) => axes is not null && axes.Contains(axis);
        return new HidMaestroCapabilities(
            HasAxisX: Has(HMAxis.X),
            HasAxisY: Has(HMAxis.Y),
            HasAxisZ: Has(HMAxis.Z),
            HasAxisRx: Has(HMAxis.Rx),
            HasAxisSlider: Has(HMAxis.Slider),
            ButtonCount: profile.ButtonCount,
            HasHat: profile.HasHat);
    }

    /// <summary>ButtonCount se mask: device par jitne buttons hain, utne hi bits (raw-report path).</summary>
    public uint ButtonMask =>
        ButtonCount <= 0 ? 0u :
        ButtonCount >= 32 ? 0xFFFF_FFFFu :
        (1u << ButtonCount) - 1u;

    /// <summary>Buttons 33..64 (raw report bytes 14..17) ke liye mask.</summary>
    public uint Ex1Mask =>
        ButtonCount <= 32 ? 0u :
        ButtonCount >= 64 ? 0xFFFF_FFFFu :
        (1u << (ButtonCount - 32)) - 1u;

    /// <summary>Buttons 65..96 (raw report bytes 18..21) ke liye mask.</summary>
    public uint Ex2Mask =>
        ButtonCount <= 64 ? 0u :
        ButtonCount >= 96 ? 0xFFFF_FFFFu :
        (1u << (ButtonCount - 64)) - 1u;
}
