namespace ApexLink.Core.Output;

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

    void Disconnect();

    /// <summary>Har tick ki nayi halat bhejo (200 Hz par bulaya jata hai).</summary>
    void Update(in ProcessedState state);

    /// <summary>Mehfooz neutral halat bhejo (watchdog / band karte waqt).</summary>
    void SendNeutral();
}
