// Linear-light gamma and contrast remap for grayscale glyph masks blended onto an sRGB surface.
float correct_glyph_alpha(float coverage, float3 rgb, float gamma, float contrast) {
    if (coverage <= 0.0f || coverage >= 1.0f) {
        return saturate(coverage);
    }
    float luma = dot(rgb, float3(0.2126f, 0.7152f, 0.0722f));
    float inv_gamma = 1.0f / max(gamma, 1.0f);
    float dark_exp = (1.0f + contrast * 0.5f) * inv_gamma;
    float dark_alpha = 1.0f - pow(1.0f - coverage, dark_exp);
    float light_alpha = pow(coverage, inv_gamma);
    return lerp(dark_alpha, light_alpha, luma);
}
