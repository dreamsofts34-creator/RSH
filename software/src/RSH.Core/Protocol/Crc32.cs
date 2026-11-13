// CRC-32/ISO-HDLC (poly 0xEDB88320, init 0xFFFFFFFF, xorout 0xFFFFFFFF).
// Firmware (config_transaction.h rsh_crc32) ke saath bilkul ek jaisa hona
// lazmi hai — config blob ka trailer (§3.6) isi se banta hai.
// Standard check: CRC32("123456789") == 0xCBF43926, CRC32("") == 0x00000000
// (host tests test_main.cpp:1155-1156 wahi kehte hain).
namespace RSH.Core.Protocol;

public static class Crc32
{
    private static readonly uint[] Table = BuildTable();

    private static uint[] BuildTable()
    {
        var table = new uint[256];
        for (uint i = 0; i < 256; i++)
        {
            uint c = i;
            for (int bit = 0; bit < 8; bit++)
                c = (c & 1) != 0 ? 0xEDB88320u ^ (c >> 1) : c >> 1;
            table[i] = c;
        }
        return table;
    }

    public static uint Compute(ReadOnlySpan<byte> data)
    {
        uint crc = 0xFFFFFFFFu;
        foreach (byte b in data)
            crc = Table[(crc ^ b) & 0xFF] ^ (crc >> 8);
        return crc ^ 0xFFFFFFFFu;
    }
}
