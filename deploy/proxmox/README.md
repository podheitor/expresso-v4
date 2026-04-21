# Proxmox Helpers (Expresso V4)

Scripts auxiliares para descobrir, recuperar e operar as VMs `expresso-*` no host Proxmox.

## Pré-requisitos

- `ssh`
- `sshpass`
- Acesso SSH ao host Proxmox

## Variáveis

```bash
export PROXMOX_HOST=192.168.194.101
export PROXMOX_USER=root
export PROXMOX_PASS='***'
```

## Fluxo rápido

1. Iniciar VMs:

```bash
./deploy/proxmox/start_expresso_vms.sh
```

2. Descobrir status/MAC/IP:

```bash
./deploy/proxmox/discover_expresso_vms.sh
```

3. Definir IP estático por Cloud-Init (se necessário):

```bash
./deploy/proxmox/set_expresso_static_ips.sh
```

4. Rebuild completo com Debian cloud image (caso VM esteja sem disco bootável):

```bash
./deploy/proxmox/rebuild_expresso_from_debian_cloud.sh
```

Esse script preserva VMIDs (`122-126`) e reprovisiona `scsi0` com Debian cloud, mantendo Cloud-Init.

5. Provisionamento base de pacotes no guest:

```bash
./deploy/proxmox/provision_expresso_base.sh
```

6. Executar comando em VM via jump no Proxmox:

```bash
./deploy/proxmox/ssh_vm_via_proxmox.sh 192.168.15.125 'hostname && ip -4 -brief a'
```

## Observações

- Em 17/04/2026, as VMs `122-126` foram recuperadas de estado `non-bootable disk`.
- Alguns containers `latest` do compose podem falhar em hosts sem suporte completo a `x86-64-v2`.
