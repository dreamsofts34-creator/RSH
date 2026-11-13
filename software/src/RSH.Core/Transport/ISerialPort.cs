// RSH wired USB-serial transport — serial port abstraction (Phase G,
// D26 / OPEN-20). Asal port System.IO.Ports.SerialPort hai
// (SystemSerialPortAdapter); tests ke liye in-memory fake port
// (RSH.Tests: LoopbackSerialPort) isi interface par chalta hai —
// SerialLink ko asal port ki zaroorat nahi hoti.
using System.IO.Ports;

namespace RSH.Core.Transport;

/// <summary>
/// Serial port ka darwaza — SerialLink is se baat karta hai, asal port
/// ya test fake se use gharaz nahi. Read timeout par 0 return karta hai
/// (koi data nahi) — TimeoutException bahar nahi aati.
/// </summary>
public interface ISerialPort : IDisposable
{
    string PortName { get; }
    bool IsOpen { get; }
    int ReadTimeout { get; set; }
    int WriteTimeout { get; set; }
    void Open();
    void Close();
    /// <summary>Bytes parho; timeout par 0. Disconnect/error par IOException ya InvalidOperationException.</summary>
    int Read(byte[] buffer, int offset, int count);
    void Write(byte[] buffer, int offset, int count);
    void DiscardInBuffer();
}

/// <summary>
/// Asal serial port (System.IO.Ports.SerialPort): 921600 baud, 8N1,
/// no handshake, no flow control.
/// DTR/RTS KABHI assert nahi hote (DtrEnable/RtsEnable = false):
/// DevKit V1 par DTR/RTS EN/BOOT lines se jure hote hain — assert karne
/// par board reset ho jata hai ya bootloader mein chala jata hai.
/// Is liye port kholna board ko reset NAHI karta (flashing tool jaisa
/// behaviour nahi — transport sirf data bhejta hai).
/// </summary>
public sealed class SystemSerialPortAdapter : ISerialPort
{
    private readonly SerialPort _port;
    private bool _disposed;

    public SystemSerialPortAdapter(string portName, int baudRate = Protocol.SerialFrameCodec.DefaultBaudRate)
    {
        ArgumentNullException.ThrowIfNull(portName);
        if (baudRate <= 0) throw new ArgumentOutOfRangeException(nameof(baudRate), "Baud rate positive hona chahiye.");
        _port = new SerialPort(portName, baudRate, Parity.None, 8, StopBits.One)
        {
            Handshake = Handshake.None,
            DtrEnable = false,
            RtsEnable = false,
            ReadTimeout = 250,
            WriteTimeout = 1000,
        };
    }

    /// <summary>System ke serial ports (Windows: COMn; Linux: /dev/ttyUSB*, /dev/ttyACM*, /dev/ttyS*). /dev/pts/* (rig pty) is mein nahi aata — rig explicit path deta hai.</summary>
    public static string[] GetPortNames() => SerialPort.GetPortNames();

    public string PortName => _port.PortName;
    public bool IsOpen => _port.IsOpen;
    public int ReadTimeout { get => _port.ReadTimeout; set => _port.ReadTimeout = value; }
    public int WriteTimeout { get => _port.WriteTimeout; set => _port.WriteTimeout = value; }

    public void Open() => _port.Open();
    public void Close() => _port.Close();

    public int Read(byte[] buffer, int offset, int count)
    {
        try
        {
            return _port.Read(buffer, offset, count);
        }
        catch (TimeoutException)
        {
            return 0;
        }
    }

    public void Write(byte[] buffer, int offset, int count) => _port.Write(buffer, offset, count);
    public void DiscardInBuffer() => _port.DiscardInBuffer();

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        try { if (_port.IsOpen) _port.Close(); } catch { /* band hote waqt khamoshi */ }
        _port.Dispose();
    }
}
