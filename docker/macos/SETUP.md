# macOS build VM – one-time setup

Building a macOS `.dmg` requires real macOS. We run it as a KVM-accelerated VM
([`dockur/macos`](https://hub.docker.com/r/dockurr/macos)) on this Windows host.
Apple's installer needs a few GUI clicks **once**; after that, `build.ps1`
drives the VM headlessly over SSH on every build.

> Running macOS on non-Apple hardware is against Apple's EULA. That's your call.

## Prerequisites (already verified on this machine)

- Docker Desktop in Linux-container mode.
- `/dev/kvm` exposed in containers (nested virtualization on in `.wslconfig`).
- Built-in Windows OpenSSH client (`ssh`, `scp`) and `tar`.

## Steps

### 1. Launch the VM with the web viewer

```powershell
./build.ps1 -Provision
```

This generates an SSH keypair at `docker/macos/id_ed25519`, prints the public
key (keep that terminal open), and boots the VM. The disk image is persisted to
`docker/macos/storage/`.

### 2. Install macOS

Open <http://localhost:8006> in your browser. macOS downloads and boots into the
installer automatically. Then:

1. **Disk Utility** → erase the virtual disk → quit.
2. **Reinstall macOS** → follow the prompts (~30–60 min).
3. In Setup Assistant, create an account with the **username `user`**
   (the scripts assume `user`). Pick any password.

### 3. Enable Remote Login (SSH)

In the macOS VM, open **Terminal** and run:

```bash
sudo systemsetup -setremotelogin on
```

### 4. Install the build SSH key

Paste the public key printed by step 1 (also in `docker/macos/id_ed25519.pub`)
into the macOS Terminal:

```bash
mkdir -p ~/.ssh && chmod 700 ~/.ssh
echo 'ssh-ed25519 AAAA...clatok-build' >> ~/.ssh/authorized_keys
chmod 600 ~/.ssh/authorized_keys
```

### 5. Install the toolchain

Back on the **Windows host**, push and run the provisioning script over SSH:

```powershell
scp -i docker/macos/id_ed25519 -P 2222 `
    docker/macos/provision.sh user@localhost:~/
ssh -i docker/macos/id_ed25519 -p 2222 user@localhost 'bash ~/provision.sh'
```

This installs Xcode Command Line Tools, Homebrew, Node, Rust (with both
`x86_64` and `aarch64` Apple targets), `create-dmg`, and Tauri CLI v2.

### 6. Shut the VM down (persists the disk)

```powershell
docker stop clatok-macos
```

## Done

From now on:

```powershell
./build.ps1 -Target mac    # boots the VM headless, builds, drops .dmg in dist/
```

## Troubleshooting

- **SSH never comes up** – the VM is still booting; cold boot can take minutes.
  Raise the wait with `./build.ps1 -Target mac -BootTimeoutSec 1800`, or watch
  it at <http://localhost:8006>.
- **`universal-apple-darwin` build fails** – edit
  `docker/macos/build-remote.sh` to use `--target x86_64-apple-darwin`
  (x86-only) instead of the universal target.
- **Permission denied (publickey)** – re-do step 4; confirm the account is
  named `user` and Remote Login is on.
