// Minimal HID report-descriptor parser: derives the input report size
// (bytes) from a descriptor instead of trusting a hardcoded assumption.
// The C# app assumed 26 bytes (a 128-button profile that was never
// built); the real rsh-wheel-custom descriptor is 124 bytes and yields
// 15 bytes of input report (5×16-bit axes + 32×1-bit buttons + 8-bit
// hat = 120 bits). Returns -1 for a malformed descriptor — callers
// must refuse to guess.
namespace RshHmSidecar;

/// <summary>Derives the input report size (bytes) from a HID report descriptor.</summary>
internal static class HidReportSize
{
    /// <summary>
    /// Parses <paramref name="descriptor"/> and returns the input
    /// report size in bytes (ceil of total input bits / 8, plus one
    /// byte when a Report ID is present). Returns -1 when the
    /// descriptor is malformed or truncated.
    /// </summary>
    public static int DeriveInputReportSize(ReadOnlySpan<byte> descriptor)
    {
        var inputBits = 0;
        var reportSize = 0;
        var reportCount = 0;
        var hasReportId = false;
        var i = 0;

        while (i < descriptor.Length)
        {
            var header = descriptor[i];
            i++;

            // Long item (0xFE): next byte is the data length.
            if (header == 0xFE)
            {
                if (i >= descriptor.Length)
                {
                    return -1;
                }

                var len = descriptor[i];
                i += 1 + len;
                if (i > descriptor.Length)
                {
                    return -1;
                }

                continue;
            }

            // Short item: bits 0-1 = size code (0->0, 1->1, 2->2, 3->4
            // data bytes), bits 2-3 = type, bits 4-7 = tag.
            var sizeCode = header & 0x03;
            var dataLen = sizeCode switch
            {
                0 => 0,
                1 => 1,
                2 => 2,
                _ => 4,
            };
            if (i + dataLen > descriptor.Length)
            {
                return -1;
            }

            var typeBits = (header >> 2) & 0x03;
            var tag = (header >> 4) & 0x0F;

            var value = 0u;
            for (var b = 0; b < dataLen; b++)
            {
                value |= (uint)descriptor[i + b] << (8 * b);
            }

            i += dataLen;

            switch (typeBits)
            {
                case 0: // Main item: Input (8), Output (9), Feature (11).
                    if (tag == 8)
                    {
                        inputBits += reportSize * reportCount;
                    }

                    break;
                case 1: // Global item.
                    switch (tag)
                    {
                        case 7: // Report Size.
                            reportSize = (int)value;
                            break;
                        case 8: // Report ID.
                            hasReportId = true;
                            break;
                        case 9: // Report Count.
                            reportCount = (int)value;
                            break;
                    }

                    break;
            }
        }

        if (inputBits == 0)
        {
            return -1;
        }

        var bytes = (inputBits + 7) / 8;
        return hasReportId ? bytes + 1 : bytes;
    }
}
