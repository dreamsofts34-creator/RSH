namespace RSH.App.Views;

/// <summary>
/// Change-password dialog ka nateeja — FluentWindow variant aur plain
/// classic variant dono yahi dete hain, taake caller (DiagnosticsViewModel)
/// ko variant ka farq na pare (O-1).
/// </summary>
public interface IChangePasswordDialogResult
{
    /// <summary>Kamyab change ke baad true.</summary>
    bool Succeeded { get; }

    /// <summary>Kamyab change ke baad naya password.</summary>
    string? NewPassword { get; }

    /// <summary>Board par password badal gaya lekin secure store mein save nahi ho saka — null = save theek hua.</summary>
    string? SaveWarning { get; }
}
