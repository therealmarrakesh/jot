#include "glyph_alpha.hlsl"

struct RasterVertexOutput {
    float4 position : SV_Position;
    float2 texcoord : TEXCOORD0;
};

RasterVertexOutput emoji_rasterization_vertex(uint vertexID : SV_VERTEXID)
{
    RasterVertexOutput output;
    output.texcoord = float2((vertexID << 1) & 2, vertexID & 2);
    output.position = float4(output.texcoord * 2.0f - 1.0f, 0.0f, 1.0f);
    output.position.y = -output.position.y;

    return output;
}

struct PixelInput {
    float4 position: SV_Position;
    float2 texcoord : TEXCOORD0;
};

Texture2D<float> t_layer : register(t0);
SamplerState s_layer : register(s0);

cbuffer GlyphLayerTextureParams : register(b0) {
    float4 run_color;
    float gamma;
    float grayscale_enhanced_contrast;
    float2 _pad;
};

float4 emoji_rasterization_fragment(PixelInput input): SV_Target {
    float sample = t_layer.Sample(s_layer, input.texcoord.xy).r;
    float alpha_corrected = correct_glyph_alpha(sample, run_color.rgb, gamma, grayscale_enhanced_contrast);
    float alpha = alpha_corrected * run_color.a;
    return float4(run_color.rgb * alpha, alpha);
}
