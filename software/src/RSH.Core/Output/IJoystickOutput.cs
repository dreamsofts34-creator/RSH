using RSH.Core.Pipeline;

namespace RSH.Core.Output;

/// <summary>
/// Joystick output ka darwaza. Asal implementation vJoy backend hai
/// (M3); tests FakeOutput istemal karte hain. Isi wajah se poora pipeline
/// baghair driver ke test ho jata hai.
/// </summary>
public interface IJoystickOutput : IDisposable
{
    string Name { get; }
    bool IsConnected { get; }

    /// <summary>Device hasil karo. Nakami par error mein wajah (insani zuban mein).</summary>
    bool TryConnect(out string error);

    /// <summary>
    /// Round 2 B2-05: plan ke saath connect — device ki salahiyatein
    /// plan ke enabled modules ke khilaf validate hoti hain (P12/D11:
    /// Steering ko X, Handbrake ko Slider, Buttons ko ≥1 button chahiye).
    /// Default implementation plan ko ignore karke plain TryConnect
    /// bulati hai (test fakes / purane backends); vJoy aur HIDMaestro
    /// isay implement karte hain (fail ho to device wapas + wajah).
    /// Pehle WheelEngine sirf plan-less TryConnect bulata tha, is liye
    /// P12 validation production mein kabhi chalti hi nahi thi — kami
    /// wala device connect ho jata tha aur input khamoshi se girta tha
    /// (BuildPosition caps-gating). Ab engine yehi overload istemal karta hai.
    /// </summary>
    bool TryConnect(out string error, PipelinePlan? plan) => TryConnect(out error);

    void Disconnect();

    /// <summary>Har tick ki nayi halat bhejo (200 Hz par bulaya jata hai).</summary>
    void Update(in ProcessedState state);

    /// <summary>Mehfooz neutral halat bhejo (watchdog / band karte waqt).</summary>
    void SendNeutral();
}
