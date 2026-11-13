namespace RSH.Output.VJoy;

/// <summary>
/// vJoy device ki salahiyatein (TryConnect par driver se parhi jati hain).
/// P12 (D11) validation isi ke khilaf hoti hai — configured modules ke
/// khilaf, fixed X/Y/Z ya ButtonCount ≥ 2 ki shart nahi.
/// Phase G C8: Handbrake ka axis Slider hai (HID usage 0x36) — is liye
/// <see cref="HasAxisSlider"/> bhi caps ka hissa hai. Clutch (WClutch)
/// ke bar-aks Slider ki HID usage hoti hai, is liye driver se poocha
/// ja sakta hai; clutch ka koi HID usage nahi, wo caps se bahar rehta hai.
/// </summary>
public readonly record struct VJoyCapabilities(
    bool HasAxisX,
    bool HasAxisY,
    bool HasAxisZ,
    int ButtonCount,
    bool HasAxisSlider = false)
{
    /// <summary>ButtonCount ki hadd (vJoy: 0..128).</summary>
    public const int MaxButtons = 128;

    /// <summary>
    /// Caps abhi parhe nahi gaye (connect nahi hua) — "unknown" ka matlab
    /// hai purana behaviour: sab kuch likho (koi gating nahi). Asal
    /// device par TryConnect caps parhta hai, is liye Update mein ye
    /// sirf us waqt istemal hota hai jab caps kabhi read hi nahi hue.
    /// ButtonCount 128 = poora device farz karo (Ex1/2/3 bhi full).
    /// Slider bhi farz karo (Phase G C8 — unknown = handbrake likho).
    /// </summary>
    public static VJoyCapabilities Unknown => new(true, true, true, 128, HasAxisSlider: true);

    /// <summary>ButtonCount se mask: device par jitne buttons hain, utne hi bits.</summary>
    public uint ButtonMask =>
        ButtonCount <= 0 ? 0u :
        ButtonCount >= 32 ? 0xFFFF_FFFFu :
        (1u << ButtonCount) - 1u;

    /// <summary>T11/Ex: vJoy buttons 33..64 (LButtonsEx1) ke liye mask.</summary>
    public uint Ex1Mask =>
        ButtonCount <= 32 ? 0u :
        ButtonCount >= 64 ? 0xFFFF_FFFFu :
        (1u << (ButtonCount - 32)) - 1u;

    /// <summary>T11/Ex: vJoy buttons 65..96 (LButtonsEx2) ke liye mask.</summary>
    public uint Ex2Mask =>
        ButtonCount <= 64 ? 0u :
        ButtonCount >= 96 ? 0xFFFF_FFFFu :
        (1u << (ButtonCount - 64)) - 1u;

    /// <summary>T11/Ex: vJoy buttons 97..128 (LButtonsEx3) ke liye mask.</summary>
    public uint Ex3Mask =>
        ButtonCount <= 96 ? 0u :
        ButtonCount >= 128 ? 0xFFFF_FFFFu :
        (1u << (ButtonCount - 96)) - 1u;
}
