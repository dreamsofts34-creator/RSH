using System.IO.MemoryMappedFiles;

namespace RSH.Core.Telemetry;

/// <summary>
/// Shared-memory map parhne ka seam (TELEMETRY-READER.md §2 rule 5):
/// <c>MemoryMappedFile</c> seedha istemal nahi hota, taake host tests mein
/// fake map (captured bytes) se snapshot parsing test ho sake.
/// <para>
/// <see cref="TryOpen"/> ka <paramref name="minSize"/> woh kam-az-kam
/// size hai jo caller ko chahiye (size/version check — §2 rule 4): map
/// is se chhota ho to open fail hota hai, reader kabhi ghalat offset se
/// nahi parhta. <see cref="Size"/> khule hue map ki asal capacity hai.
/// </para>
/// </summary>
public interface IMemoryMapAccessor : IDisposable
{
    /// <summary>Khula hua map (agar ho) — band ho to 0.</summary>
    int Size { get; }

    /// <summary>
    /// Naam se map kholo (pehle se khula ho to band karke dobara).
    /// Map na mile ya <paramref name="minSize"/> se chhota ho to false.
    /// Kabhi throw nahi karta.
    /// </summary>
    bool TryOpen(string mapName, int minSize);

    /// <summary>
    /// <paramref name="offset"/> se <paramref name="count"/> bytes parho —
    /// kam bytes mile to utne hi (0 = map band / offset bahir). Kabhi
    /// throw nahi karta.
    /// </summary>
    int ReadBytes(long offset, byte[] buffer, int count);
}

/// <summary>Asal Windows shared-memory accessor — read-only mmap.</summary>
public sealed class WindowsMemoryMapAccessor : IMemoryMapAccessor
{
    private MemoryMappedFile? _file;
    private MemoryMappedViewAccessor? _view;

    public int Size => _view is null ? 0 : (int)Math.Min(_view.Capacity, int.MaxValue);

    public bool TryOpen(string mapName, int minSize)
    {
        Close();
        // CA1416: MemoryMappedFile.OpenExisting Windows-only hai — Linux
        // (rig/test VM) par map kabhi nahi milta, is liye false (caller
        // ise NotInSession samajhta hai). IsWindows guard codebase ka
        // established pattern hai (UpdateCheck / FirewallRule).
        if (!OperatingSystem.IsWindows()) return false;
        try
        {
            // OpenExisting: map na ho to FileNotFoundException — game
            // band / menu mein hai, ye normal hai (NotInSession).
            _file = MemoryMappedFile.OpenExisting(mapName, MemoryMappedFileRights.Read);
            _view = _file.CreateViewAccessor(0, 0, MemoryMappedFileAccess.Read);
            if (_view.Capacity < minSize)
            {
                Close();
                return false;
            }
            return true;
        }
        catch (Exception)
        {
            Close();
            return false;
        }
    }

    public int ReadBytes(long offset, byte[] buffer, int count)
    {
        if (_view is null || buffer is null || count <= 0 || offset < 0) return 0;
        try
        {
            long available = _view.Capacity - offset;
            if (available <= 0) return 0;
            int toRead = (int)Math.Min(Math.Min(available, count), buffer.Length);
            _view.ReadArray(offset, buffer, 0, toRead);
            return toRead;
        }
        catch (Exception)
        {
            return 0;
        }
    }

    private void Close()
    {
        try { _view?.Dispose(); } catch (Exception) { /* bandish par khamoshi */ }
        try { _file?.Dispose(); } catch (Exception) { /* bandish par khamoshi */ }
        _view = null;
        _file = null;
    }

    public void Dispose() => Close();
}

/// <summary>
/// Test seam (TELEMETRY-READER.md §4): naam → captured bytes. Har
/// <see cref="TryOpen"/> naam se map chunta hai; na mile ya
/// <c>minSize</c> se chhota ho to false — bilkul asal accessor jaisa.
/// </summary>
public sealed class FakeMemoryMapAccessor : IMemoryMapAccessor
{
    private readonly Dictionary<string, byte[]> _maps;
    private byte[]? _current;

    public FakeMemoryMapAccessor(Dictionary<string, byte[]> maps) =>
        _maps = new Dictionary<string, byte[]>(maps, StringComparer.Ordinal);

    /// <summary>Ek map wala shortcut (aksar test mein ek hi map chahiye hota hai).</summary>
    public FakeMemoryMapAccessor(string mapName, byte[] bytes) : this(new Dictionary<string, byte[]> { [mapName] = bytes })
    {
    }

    public int Size => _current?.Length ?? 0;

    public bool TryOpen(string mapName, int minSize)
    {
        _current = null;
        if (!_maps.TryGetValue(mapName, out byte[]? bytes)) return false;
        if (bytes.Length < minSize) return false;
        _current = bytes;
        return true;
    }

    public int ReadBytes(long offset, byte[] buffer, int count)
    {
        if (_current is null || buffer is null || count <= 0 || offset < 0) return 0;
        if (offset >= _current.Length) return 0;
        int toRead = (int)Math.Min(Math.Min((long)_current.Length - offset, count), buffer.Length);
        Array.Copy(_current, (int)offset, buffer, 0, toRead);
        return toRead;
    }

    public void Dispose() => _current = null;
}
