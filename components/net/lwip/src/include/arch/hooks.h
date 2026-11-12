// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#ifndef ARONA_LWIP_HOOKS_H
#define ARONA_LWIP_HOOKS_H
#include "lwip/ip_addr.h"
uint32_t arona_lwip_isn(const uint8_t *local, uint16_t local_port,
                       const uint8_t *remote, uint16_t remote_port);
#define LWIP_HOOK_TCP_ISN(local, local_port, remote, remote_port) \
  arona_lwip_isn((const uint8_t *)&ip_2_ip4(local)->addr, local_port, \
                 (const uint8_t *)&ip_2_ip4(remote)->addr, remote_port)
#endif
