namespace RSH.Core.Telemetry;

/// <summary>
/// Reader contract (TELEMETRY-READER.md §4): ek game, ek poll.
/// <see cref="TryRead"/> kabhi throw nahi karta — game na ho, map na
/// mile, ya bytes kharab hon to <c>false</c> (snapshot default).
/// </summary>
public interface ITelemetryReader
{
    GameId Game { get; }

    bool TryRead(out GameTelemetrySnapshot snapshot);
}
