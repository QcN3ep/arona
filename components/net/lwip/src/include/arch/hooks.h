// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#ifndef ARONA_LWIP_HOOKS_H
#define ARONA_LWIP_HOOKS_H
#include "lwip/ip_addr.h"
uint32_t arona_lwip_isn(const uint8_t *local, uint16_t local_port,
                       const uint8_t *remote, uint16_t remote_port, size_t length);
#define LWIP_HOOK_TCP_ISN(local, local_port, remote, remote_port) \
  arona_lwip_isn((const uint8_t *)(local), local_port, \
                 (const uint8_t *)(remote), remote_port, IP_IS_V6(local) ? 16 : 4)
#endif
