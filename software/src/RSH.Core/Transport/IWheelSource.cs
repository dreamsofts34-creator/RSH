// RSH wheel source abstraction (Phase G, D26 / OPEN-20) — Wi-Fi
// (UdpWheelSource) aur USB-serial (SerialWheelSource) dono ka aik hi
// darwaza. WheelEngine, ViewModels aur AppServices is se baat karte
// hain — transport switch par engine/pipeline dobara bante hain, lekin
// un ka code transport se be-khabar rehta hai (koi duplicate engine nahi).
using RSH.Core.Protocol;

namespace RSH.Core.Transport;

/// <summary>
/// Wheel source: taaza packet slot (v1 + v2.1) + status + HELLO.
/// UdpWheelSource aur SerialWheelSource dono implement karte hain.
/// </summary>
public interface IWheelSource : ISampleSource, IV2SampleSource, IStatusSource, IDisposable
{
    /// <summary>Source start karo (socket bind / port kholo + receive loop).</summary>
    void Start();
    /// <summary>Source roko (receive loop band; port/socket band karna Dispose ki zimmedari ho sakti hai — dekho implementation).</summary>
    void Stop();
    /// <summary>Valid HELLO reply aaya — BoardSession is se nonce/key leta hai.</summary>
    event Action<HelloReply>? HelloReplyReceived;
    /// <summary>UI/diagnostics ke liye: "UDP port 4211" ya "Serial COM3" jaisi pehchan.</summary>
    string SourceDescription { get; }
}
