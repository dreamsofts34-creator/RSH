// RSH — ChangePasswordDialog variant picker (O-1, Round 2 W2).
// WPF-free (TransportSwitchController jaisa): classic mode (Win10 auto)
// mein plain-Window variant khulta hai, Win11 par FluentWindow dialog
// waisa hi rehta hai. Ye file RSH.Tests mein link hoti hai
// (RSH.Tests.csproj), taake selection logic Linux par test ho sake —
// asal Window kholna Windows-only hai (RSH.App.Tests, type-level).
namespace RSH.App.Services;

/// <summary>ChangePasswordDialog ki kaunsi variant khulegi.</summary>
public enum ChangePasswordDialogKind
{
    /// <summary>Win11 default — WPF-UI FluentWindow dialog.</summary>
    Fluent,
    /// <summary>Classic mode (Win10 auto / --classic-window) — plain System.Windows.Window.</summary>
    Classic,
}

/// <summary>
/// Classic mode mein FluentWindow dialog modal hang jaisa lag sakta hai
/// (wahi chrome family jo blank-window ka suspect thi) — is liye
/// classic mode mein plain-Window variant select hoti hai.
/// </summary>
public static class ChangePasswordDialogPicker
{
    /// <summary>Classic window requested ho to Classic, warna Fluent (Win11 unchanged).</summary>
    public static ChangePasswordDialogKind Pick(bool classicWindowRequested) =>
        classicWindowRequested ? ChangePasswordDialogKind.Classic : ChangePasswordDialogKind.Fluent;
}
