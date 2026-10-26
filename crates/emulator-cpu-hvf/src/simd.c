/* HVF's vector-by-value setter uses the C SIMD ABI. */
#include <Hypervisor/Hypervisor.h>
#include <string.h>
hv_return_t camera_hvf_get_simd(hv_vcpu_t cpu, uint32_t reg, unsigned char *bytes) {
    hv_simd_fp_uchar16_t value;
    hv_return_t status = hv_vcpu_get_simd_fp_reg(cpu, reg, &value);
    if (status == HV_SUCCESS) memcpy(bytes, &value, sizeof(value));
    return status;
}
hv_return_t camera_hvf_set_simd(hv_vcpu_t cpu, uint32_t reg, const unsigned char *bytes) {
    hv_simd_fp_uchar16_t value;
    memcpy(&value, bytes, sizeof(value));
    return hv_vcpu_set_simd_fp_reg(cpu, reg, value);
}
