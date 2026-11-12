// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#ifndef ARONA_LWIP_STDLIB_H
#define ARONA_LWIP_STDLIB_H
int arona_lwip_atoi(const char *text);
#define atoi arona_lwip_atoi
#endif
