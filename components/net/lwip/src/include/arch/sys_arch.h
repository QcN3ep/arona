// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#ifndef ARONA_LWIP_SYS_ARCH_H
#define ARONA_LWIP_SYS_ARCH_H

typedef void *sys_sem_t;
typedef void *sys_mutex_t;
typedef void *sys_mbox_t;
typedef unsigned long sys_thread_t;
typedef unsigned int sys_prot_t;

#define SYS_MBOX_NULL NULL
#define SYS_SEM_NULL NULL
#define LWIP_COMPAT_MUTEX 1
#define sys_sem_valid(sem) (*(sem) != NULL)
#define sys_sem_set_invalid(sem) (*(sem) = NULL)
#define sys_mbox_valid(mbox) (*(mbox) != NULL)
#define sys_mbox_set_invalid(mbox) (*(mbox) = NULL)

#endif
