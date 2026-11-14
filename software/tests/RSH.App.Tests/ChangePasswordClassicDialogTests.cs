using System.Windows;
using RSH.App.Views;

namespace RSH.App.Tests;

/// <summary>
/// O-1 (Round 2 W2) — ChangePasswordDialog classic variant: plain
/// System.Windows.Window honi chahiye (FluentWindow nahi), aur Fluent
/// variant Win11 par waisi hi rehni chahiye. Windows-only (WPF types) —
/// Linux par RSH.Tests mein picker + XAML-text tests chalte hain.
/// Koi instance nahi banti (STA/dispatcher ki zaroorat nahi — sirf type lineage).
/// </summary>
public sealed class ChangePasswordClassicDialogTests
{
    [Fact]
    public void ClassicDialog_IsPlainWindow_NotFluentWindow()
    {
        Assert.Equal(typeof(Window), typeof(ChangePasswordDialogClassic).BaseType);
        Assert.True(typeof(Window).IsAssignableFrom(typeof(ChangePasswordDialogClassic)));
    }

    [Fact]
    public void FluentDialog_Unchanged_StillFluentWindow()
    {
        Assert.Equal(typeof(Wpf.Ui.Controls.FluentWindow), typeof(ChangePasswordDialog).BaseType);
    }

    [Fact]
    public void BothDialogs_ShareResultContract()
    {
        Assert.True(typeof(IChangePasswordDialogResult).IsAssignableFrom(typeof(ChangePasswordDialogClassic)));
        Assert.True(typeof(IChangePasswordDialogResult).IsAssignableFrom(typeof(ChangePasswordDialog)));
    }
}
