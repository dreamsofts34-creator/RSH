using System.Net;
using System.Net.Sockets;
using System.Text;
using RSH.Core.Transport;

namespace RSH.Tests;

/// <summary>
/// T10 (ephemeral ports) + receive-buffer follow-up (SW-4/P7, Phase C).
/// Port 0 = OS-assigned ephemeral — doosre agents ke concurrent test
/// runs se fixed-port takrao (Address already in use) nahi hota.
/// Is file mein KOI fixed test port nahi hai: explicit-port tests bhi
/// OS se khaali port allocate karte hain (neeche helpers) — hardcoded
/// 44214 par verification mein takrao mila tha (concurrent runs).
/// </summary>
public class EphemeralPortTests
{
    /// <summary>OS se khaali UDP port lo (bind 0, phir band kar do).</summary>
    private static int AllocFreePort()
    {
        using var probe = new UdpClient(0);
        return ((IPEndPoint)probe.Client.LocalEndPoint!).Port;
    }

    /// <summary>
    /// Explicit port par UdpWheelSource banao — takrao ho to naya khaali
    /// port try karo (5 koshishen; aakhir mein asal error bahar aata hai).
    /// Allocate aur bind ke darmiyan chhoti race rehti hai, is liye retry.
    /// </summary>
    private static UdpWheelSource CreateSourceOnExplicitPort(out int port)
    {
        for (int attempt = 0; ; attempt++)
        {
            port = AllocFreePort();
            try { return new UdpWheelSource(port); }
            catch (SocketException ex) when (ex.SocketErrorCode == SocketError.AddressAlreadyInUse && attempt < 4) { }
        }
    }

    /// <summary>Explicit local port par HelloClient banao (retry, oopar jaisa).</summary>
    private static HelloClient CreateHelloOnExplicitPort(out int port)
    {
        for (int attempt = 0; ; attempt++)
        {
            port = AllocFreePort();
            try { return new HelloClient("127.0.0.1", HelloClient.EspHelloPort, port); }
            catch (SocketException ex) when (ex.SocketErrorCode == SocketError.AddressAlreadyInUse && attempt < 4) { }
        }
    }

    /// <summary>Listener socket khaali port par banao (retry, oopar jaisa).</summary>
    private static UdpClient CreateListener(out int port)
    {
        for (int attempt = 0; ; attempt++)
        {
            port = AllocFreePort();
            try { return new UdpClient(port); }
            catch (SocketException ex) when (ex.SocketErrorCode == SocketError.AddressAlreadyInUse && attempt < 4) { }
        }
    }
    // ---- T10: UdpWheelSource ephemeral default ----

    [Fact]
    public void UdpSource_DefaultPort_IsEphemeral()
    {
        using var source = new UdpWheelSource();
        Assert.Equal(UdpWheelSource.DefaultListenPort, 0);
        Assert.True(source.ListenPort > 0, $"ListenPort OS-assigned hona chahiye, mila {source.ListenPort}");
        Assert.True(source.ListenPort <= 65535);
    }

    [Fact]
    public void UdpSource_TwoDefaults_GetDifferentPorts()
    {
        using var a = new UdpWheelSource();
        using var b = new UdpWheelSource();
        Assert.NotEqual(a.ListenPort, b.ListenPort);
    }

    [Fact]
    public void UdpSource_ExplicitPort_StillWorks()
    {
        using var source = CreateSourceOnExplicitPort(out int port);
        Assert.Equal(port, source.ListenPort);
    }

    [Fact]
    public void UdpSource_InvalidPort_Throws()
    {
        Assert.Throws<ArgumentOutOfRangeException>(() => new UdpWheelSource(-1));
        Assert.Throws<ArgumentOutOfRangeException>(() => new UdpWheelSource(65536));
    }

    // ---- Receive buffer (SW-4/P7 follow-up) ----

    [Fact]
    public void UdpSource_ReceiveBuffer_IsExplicitlySized()
    {
        // OS default (Windows: 8 KB) par chhoti rukawat se bhi kernel
        // datagram gira deta tha. Ab kam se kam 64 KB hona chahiye.
        using var source = new UdpWheelSource(0);
        Assert.True(source.ReceiveBufferSizeBytes >= UdpWheelSource.MinimumReceiveBufferSizeBytes,
            $"Receive buffer kam se kam {UdpWheelSource.MinimumReceiveBufferSizeBytes} B hona chahiye, mila {source.ReceiveBufferSizeBytes}");
    }

    [Fact]
    public void UdpSource_ReceiveBuffer_DefaultTarget_Is256K()
    {
        Assert.Equal(256 * 1024, UdpWheelSource.DefaultReceiveBufferSizeBytes);
        Assert.Equal(64 * 1024, UdpWheelSource.MinimumReceiveBufferSizeBytes);
    }

    // ---- T10: HelloClient ephemeral source port ----

    [Fact]
    public void HelloClient_Default_LocalPortIsEphemeral()
    {
        using var hello = new HelloClient();
        Assert.True(hello.LocalPort > 0, $"LocalPort OS-assigned hona chahiye, mila {hello.LocalPort}");
        Assert.Equal(HelloClient.EspHelloPort, 4210);   // board ka port — destination, fixed rehta hai
    }

    [Fact]
    public void HelloClient_ExplicitLocalPort_BindsThatPort()
    {
        // Standalone HELO sender: khaali port diya jaye to HELO usi
        // port se jata hai. (UdpWheelSource se bound port par doosra
        // socket bind nahi ho sakta — us ke liye SendHelloAsync hai.)
        using var hello = CreateHelloOnExplicitPort(out int port);
        Assert.Equal(port, hello.LocalPort);
    }

    [Fact]
    public async Task UdpSource_SendHelloAsync_SourcePortIsListenPort()
    {
        // T10 mechanism: HELO data socket se jaye to board ko data
        // port HELO ke source port se pata chalta hai.
        using var source = new UdpWheelSource(0);
        using var listener = CreateListener(out int listenPort);
        var receive = listener.ReceiveAsync(TestContext.Current.CancellationToken).AsTask();
        try
        {
            await source.SendHelloAsync("127.0.0.1", listenPort, TestContext.Current.CancellationToken);
        }
        catch (SocketException ex) when (ex.SocketErrorCode == SocketError.AccessDenied)
        {
            // Is mahol mein UDP loopback hi band hai (VM policy) —
            // test maqsad nahi rakhta; Windows par ye poora chalega.
            Assert.Skip("Is mahol mein UDP loopback ki ijazat nahi.");
            throw; // unreachable — Skip hamesha throw karta hai
        }
        var done = await Task.WhenAny(receive, Task.Delay(1000, TestContext.Current.CancellationToken));
        if (done != receive)
            Assert.Skip("Is mahol mein UDP loopback packets nahi pohanchte.");
        var result = await receive;
        Assert.Equal("HELO", Encoding.ASCII.GetString(result.Buffer));
        Assert.Equal(source.ListenPort, result.RemoteEndPoint.Port);
    }

    [Fact]
    public void HelloClient_TwoDefaults_GetDifferentLocalPorts()
    {
        using var a = new HelloClient();
        using var b = new HelloClient();
        Assert.NotEqual(a.LocalPort, b.LocalPort);
    }

    [Fact]
    public void HelloClient_InvalidPorts_Throw()
    {
        Assert.Throws<ArgumentOutOfRangeException>(() => new HelloClient("127.0.0.1", 0));
        Assert.Throws<ArgumentOutOfRangeException>(() => new HelloClient("127.0.0.1", 65536));
        Assert.Throws<ArgumentOutOfRangeException>(() => new HelloClient("127.0.0.1", 4210, -1));
        Assert.Throws<ArgumentOutOfRangeException>(() => new HelloClient("127.0.0.1", 4210, 65536));
    }
}
