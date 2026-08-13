import playground;
import rendering;

[playground::TIME]
uniform float time;

[format("r32f")]
[playground::BLACK_3D(64, 64, 48)]
RWTexture3D<float> volume;

[shader("compute")]
[numthreads(4, 4, 4)]
[playground::CALL::SIZE_OF("volume")]
[playground::CALL::ONCE]
void buildVolume(uint3 dispatchThreadID : SV_DispatchThreadID)
{
    uint width, height, depth;
    volume.GetDimensions(width, height, depth);

    if (dispatchThreadID.x >= width || dispatchThreadID.y >= height || dispatchThreadID.z >= depth)
        return;

    float3 uvw = float3(dispatchThreadID) / float3(width, height, depth);
    float wave = sin(uvw.x * 6.283) * cos(uvw.y * 6.283) * sin(uvw.z * 6.283);
    volume[dispatchThreadID] = 0.5 * (wave + 1.0);
}

[shader("compute")]
[numthreads(16, 16, 1)]
[playground::CALL::SIZE_OF("outputTexture")]
void visualize(uint2 dispatchThreadID : SV_DispatchThreadID)
{
    drawPixel(dispatchThreadID, (int2 screenSize) => {
        if (dispatchThreadID.x >= screenSize.x || dispatchThreadID.y >= screenSize.y)
            return float4(0, 0, 0, 1);

        uint width, height, depth;
        volume.GetDimensions(width, height, depth);

        float slice = frac(time * 0.2) * (depth - 1);
        float2 denom = float2(max(screenSize.x - 1, 1), max(screenSize.y - 1, 1));
        float2 uv = float2(dispatchThreadID) / denom;

        int sampleX = clamp(int(round(uv.x * (width  - 1))), 0, int(width  - 1));
        int sampleY = clamp(int(round(uv.y * (height - 1))), 0, int(height - 1));
        int sampleZ = clamp(int(round(slice)), 0, int(depth - 1));

        float value = volume.Load(int3(sampleX, sampleY, sampleZ));
        float3 color = float3(value, value * 0.6, 1.0 - value);
        return float4(color, 1.0);
    });
}
