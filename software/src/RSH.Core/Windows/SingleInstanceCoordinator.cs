using System.IO.Pipes;

namespace RSH.Core.Windows;

/// <summary>
/// Single instance + bring-to-front (A4, SW-10).
/// <para>
/// Mutex <c>Global\RSH.SingleInstance</c> pehli instance ki pehchan hai;
/// doosri instance named pipe par "activate" bhej kar pehli ko samne lati
/// hai, phir khud band ho jati hai. Named pipe is liye: named
/// <c>EventWaitHandle</c> Linux par <c>PlatformNotSupportedException</c>
/// deta hai (sandbox mein verify kiya), jab ke named pipe + named mutex
/// dono platforms par chalte hain — is liye yehi mechanism unit test mein
/// bhi verify ho sakta hai.
/// </para>
/// <para>Windows par asal bring-to-front (window restore + foreground)
/// caller ki zimmedari hai — <see cref="StartListening"/> ka callback UI
/// thread par marshal karke window ko activate kare.</para>
/// </summary>
public sealed class SingleInstanceCoordinator : IDisposable
{
    public const string DefaultMutexName = @"Global\RSH.SingleInstance";
    public const string DefaultPipeName = "RSH.SingleInstance";

    private readonly string _mutexName;
    private readonly string _pipeName;
    private Mutex? _mutex;
    private CancellationTokenSource? _listenCts;
    private Task? _listenTask;
    private bool _disposed;

    public SingleInstanceCoordinator(string mutexName = DefaultMutexName, string pipeName = DefaultPipeName)
    {
        _mutexName = mutexName;
        _pipeName = pipeName;
    }

    /// <summary>
    /// Mutex hasil karo. <c>true</c> = ye pehli instance hai (aage chalao);
    /// <c>false</c> = pehli instance pehle se chal rahi hai.
    /// </summary>
    public bool TryAcquire()
    {
        ObjectDisposedException.ThrowIf(_disposed, this);
        _mutex = new Mutex(initiallyOwned: true, _mutexName, out bool createdNew);
        return createdNew;
    }

    /// <summary>Pehli instance mein activation listener shuru karo (background).</summary>
    public void StartListening(Action onActivate)
    {
        ArgumentNullException.ThrowIfNull(onActivate);
        ObjectDisposedException.ThrowIf(_disposed, this);
        if (_mutex is null) throw new InvalidOperationException("Pehle TryAcquire() chalao.");
        _listenCts = new CancellationTokenSource();
        var token = _listenCts.Token;
        _listenTask = Task.Run(async () =>
        {
            while (!token.IsCancellationRequested)
            {
                try
                {
                    using var server = new NamedPipeServerStream(
                        _pipeName, PipeDirection.In, 1, PipeTransmissionMode.Byte, PipeOptions.Asynchronous);
                    await server.WaitForConnectionAsync(token).ConfigureAwait(false);
                    using var reader = new StreamReader(server);
                    await reader.ReadLineAsync(token).ConfigureAwait(false);
                    if (!token.IsCancellationRequested)
                        onActivate();
                }
                catch (OperationCanceledException) { break; }
                catch (IOException) { /* client beech mein toot gaya — agla connection suno */ }
                catch (ObjectDisposedException) { break; }
            }
        }, token);
    }

    /// <summary>Chal rahi pehli instance ko activate karne ka signal bhejo (best-effort).</summary>
    public static bool SignalExisting(string pipeName = DefaultPipeName, int timeoutMs = 1000)
    {
        try
        {
            using var client = new NamedPipeClientStream(".", pipeName, PipeDirection.Out);
            client.Connect(timeoutMs);
            using var writer = new StreamWriter(client) { AutoFlush = true };
            writer.WriteLine("activate");
            return true;
        }
        catch
        {
            // Pehli instance ka pipe na mile (race: wo abhi band ho rahi ho)
            // to signal nahi bhej sakte — doosri instance phir bhi band hoti hai.
            return false;
        }
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        try { _listenCts?.Cancel(); } catch { /* bandish par khamoshi */ }
        try { _listenTask?.Wait(TimeSpan.FromSeconds(2)); } catch { /* bandish par khamoshi */ }
        _listenCts?.Dispose();
        _mutex?.Dispose();
    }
}
