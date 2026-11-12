using ApexLink.Core.Protocol;

namespace ApexLink.Core.Transport;

/// <summary>
/// Packet source ka darwaza — engine ko is se gharaz nahi ke packets
/// UDP se aye ya test ke nakli source se. UdpWheelSource asal hai.
/// </summary>
public interface ISampleSource
{
    LinkStatistics Statistics { get; }
    bool TryGetLatest(out WheelPacket packet, out long ageMs, long maxAgeMs = long.MaxValue);
    long? LastValidAgeMs { get; }
}
