#ifndef FUJIMOD_EXR2_RESTART_FACTS_H
#define FUJIMOD_EXR2_RESTART_FACTS_H

/* EXR II serial controller and GPIO register map. */
#define FM_EXR2_RESTART_SPI_BASE             0xfff84040u
#define FM_EXR2_RESTART_SPI_CONTROL          0x00u
#define FM_EXR2_RESTART_SPI_FORMAT           0x04u
#define FM_EXR2_RESTART_SPI_FIFO_CONTROL     0x08u
#define FM_EXR2_RESTART_SPI_STATUS           0x0cu
#define FM_EXR2_RESTART_SPI_RX_COUNT         0x10u
#define FM_EXR2_RESTART_SPI_TX_COUNT         0x14u
#define FM_EXR2_RESTART_SPI_RX               0x18u
#define FM_EXR2_RESTART_SPI_TX               0x1cu
#define FM_EXR2_RESTART_SPI_ENABLE           0x80u
#define FM_EXR2_RESTART_SPI_BUSY             0x01u
#define FM_EXR2_RESTART_SPI_FIFO_RESET       0x10000000u
#define FM_EXR2_RESTART_SPI_STATUS_CLEAR     0x3311u
#define FM_EXR2_RESTART_SPI_RX_READY         0x200u
#define FM_EXR2_RESTART_SPI_COUNT_MASK       0x1fu
#define FM_EXR2_RESTART_GPIO_BASE            0xfff89000u
#define FM_EXR2_RESTART_GPIO_STRIDE          0x40u
#define FM_EXR2_RESTART_GPIO_BANKS           16u
#define FM_EXR2_RESTART_GPIO_OUTPUT          0x00u
#define FM_EXR2_RESTART_GPIO_MODE1           0x04u
#define FM_EXR2_RESTART_GPIO_MODE2           0x08u
#define FM_EXR2_RESTART_GPIO_MODE3           0x0cu
#define FM_EXR2_RESTART_GPIO_MODE4           0x10u
#define FM_EXR2_RESTART_GPIO_CONFIG          0x18u
#define FM_EXR2_RESTART_POWER_HOLD           0xfff88084u
#define FM_EXR2_RESTART_POWER_HOLD_MASK      0x01u

/* Power-controller SPI register protocol and alarm commands. */
#define FM_EXR2_RESTART_WRITE                0x8000u
#define FM_EXR2_RESTART_PACKET_REGISTER      0x50u
#define FM_EXR2_RESTART_PACKET_BYTES         16u
#define FM_EXR2_RESTART_ALARM_LOW            0x69u
#define FM_EXR2_RESTART_ALARM_HIGH           0x6au
#define FM_EXR2_RESTART_ALARM_ENABLE         0x68u
#define FM_EXR2_RESTART_POWER_COMMAND        0x60u
#define FM_EXR2_RESTART_POWER_OFF_VALUE      0xf0u
#define FM_EXR2_RESTART_GPIO_INACTIVE_MASK   0xffff0000u

#endif
