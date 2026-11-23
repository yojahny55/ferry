// Drives and network shares for the sidebar. Drives come from lsblk (what is plugged in, mounted
// or not, with capacity), network shares from /proc/mounts and /etc/fstab, so a share that is
// automounted but asleep still shows, without anything here touching the network. Mounting,
// ejecting and formatting go through UDisks, the same service GNOME and KDE use, so the desktop's
// polkit agent asks for a password when one is needed.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::files::human_size;
use crate::i18n::tr;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Usb,
    Drive,
    /// A disk image attached as a loop device.
    Image,
    Network,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    pub name: String,
    pub kind: Kind,
    /// Where it's mounted; for a network share, where it mounts (it may be asleep).
    pub mount: Option<PathBuf>,
    /// The block device, such as /dev/sda1; none for network shares.
    pub block: Option<String>,
    /// The whole disk to power off on eject, such as /dev/sda.
    pub disk: Option<String>,
    pub fstype: String,
    pub size: u64,
    pub used: Option<u64>,
    /// The drive's model, or a share's address.
    pub detail: String,
    /// Plugged in rather than built in: it can be ejected and formatted.
    pub removable: bool,
}

impl Device {
    pub fn icon(&self) -> char {
        match self.kind {
            Kind::Usb => '\u{f287}',
            Kind::Drive => '\u{f0a0}',
            Kind::Image => '\u{f1c0}',
            Kind::Network => '\u{f233}',
        }
    }

    /// How full it is, from 0 to 1, when that's known.
    pub fn fill(&self) -> Option<f32> {
        let used = self.used?;
        (self.size > 0).then(|| (used as f64 / self.size as f64).clamp(0.0, 1.0) as f32)
    }

    /// The hover text: what it is, how full, and where it lives.
    pub fn summary(&self) -> String {
        let mut lines = Vec::new();
        if !self.detail.is_empty() {
            lines.push(self.detail.clone());
        }
        match (self.used, self.kind) {
            (Some(used), _) => lines.push(
                tr("%1 free of %2")
                    .replace("%1", &human_size(self.size.saturating_sub(used)))
                    .replace("%2", &human_size(self.size)),
            ),
            (None, Kind::Network) => {}
            (None, _) if self.fstype.is_empty() => lines.push(tr("Not formatted")),
            (None, _) => lines.push(tr("Not mounted")),
        }
        let mut place = Vec::new();
        if !self.fstype.is_empty() {
            place.push(self.fstype.clone());
        }
        place.extend(self.block.clone());
        place.extend(self.mount.as_ref().map(|m| m.display().to_string()));
        lines.push(place.join(" · "));
        lines.join("\n")
    }
}

const NETWORK_FS: &[&str] = &[
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "smbfs",
    "fuse.sshfs",
    "sshfs",
    "davfs",
    "fuse.rclone",
    "9p",
];
/// Filesystems that aren't files: encrypted containers, swap, RAID and LVM members.
const NOT_FILES: &[&str] = &[
    "crypto_LUKS",
    "swap",
    "LVM2_member",
    "linux_raid_member",
    "zfs_member",
];
const MEDIA_DIRS: &[&str] = &["/run/media/", "/media/", "/mnt/"];

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("").trim()
}

fn u64_of(v: &Value, key: &str) -> Option<u64> {
    match v.get(key)? {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn unescape(path: &str) -> String {
    path.replace("\\040", " ").replace("\\011", "\t")
}

/// Lists the drives and shares from lsblk's JSON, /proc/mounts and /etc/fstab.
pub fn parse(lsblk: &str, mounts: &str, fstab: &str) -> Vec<Device> {
    let mut out = Vec::new();
    let root: Value = serde_json::from_str(lsblk).unwrap_or(Value::Null);
    // lsblk nests partitions under their disk, or lists everything flat with each device's
    // parent in PKNAME, depending on the columns and version: flatten both the same way.
    let mut all: Vec<(&Value, Option<String>)> = Vec::new();
    fn flatten<'a>(
        v: &'a Value,
        parent: Option<String>,
        all: &mut Vec<(&'a Value, Option<String>)>,
    ) {
        let parent = parent.or_else(|| {
            let name = str_of(v, "pkname");
            (!name.is_empty()).then(|| format!("/dev/{name}"))
        });
        all.push((v, parent));
        for child in v
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            flatten(child, Some(str_of(v, "path").to_string()), all);
        }
    }
    for dev in root
        .get("blockdevices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        flatten(dev, None, &mut all);
    }
    for (dev, parent) in &all {
        let path = str_of(dev, "path");
        // A partition takes transport, hotplug and model from its disk.
        let disk = parent
            .as_deref()
            .and_then(|pp| all.iter().find(|(d, _)| str_of(d, "path") == pp))
            .map_or(*dev, |(d, _)| *d);
        let has_parts = all.iter().any(|(_, pp)| pp.as_deref() == Some(path));
        block(dev, disk, has_parts, &mut out);
    }
    // Network shares: mounted now, or set up to mount on demand in fstab.
    let mut seen = std::collections::HashSet::new();
    let fstab_lines = fstab.lines().filter(|l| !l.trim_start().starts_with('#'));
    for line in mounts.lines().chain(fstab_lines) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let [source, target, fstype, ..] = f[..] else {
            continue;
        };
        if !NETWORK_FS.contains(&fstype) || !target.starts_with('/') {
            continue;
        }
        let target = unescape(target);
        if !seen.insert(target.clone()) {
            continue;
        }
        let path = PathBuf::from(&target);
        out.push(Device {
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(target.clone()),
            kind: Kind::Network,
            mount: Some(path),
            block: None,
            disk: None,
            fstype: fstype.to_string(),
            size: 0,
            used: None,
            detail: unescape(source),
            removable: false,
        });
    }
    out
}

/// Adds `dev` (a disk, partition or loop device) when it belongs in the sidebar. `disk` is the
/// disk it's on (or itself), which knows the transport and whether it's plugged in.
fn block(dev: &Value, disk: &Value, has_parts: bool, out: &mut Vec<Device>) {
    let kind_s = str_of(dev, "type");
    let fstype = str_of(dev, "fstype");
    if !matches!(kind_s, "part" | "disk" | "loop") || NOT_FILES.contains(&fstype) {
        return;
    }
    // A disk with partitions shows as its partitions; one without, as itself.
    if kind_s == "disk" && has_parts {
        return;
    }
    let mounts: Vec<String> = dev
        .get("mountpoints")
        .and_then(Value::as_array)
        .map(|m| {
            m.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    let mount = mounts.first().cloned();
    let in_media = mount
        .as_ref()
        .is_some_and(|m| MEDIA_DIRS.iter().any(|d| m.starts_with(d)));
    let loop_dev = kind_s == "loop" || str_of(disk, "type") == "loop";
    let usb = str_of(disk, "tran") == "usb" || str_of(dev, "tran") == "usb";
    let plugged = usb
        || [dev, disk]
            .iter()
            .any(|d| d.get("hotplug").and_then(Value::as_bool) == Some(true))
        || [dev, disk]
            .iter()
            .any(|d| d.get("rm").and_then(Value::as_bool) == Some(true));
    // Built-in disks only show when mounted where removable media go; the system's own
    // partitions (/, /boot, /home) never do. Disk images show unless something else uses them.
    let shown = if loop_dev {
        !fstype.is_empty() && (mount.is_none() || in_media)
    } else if plugged {
        mount.is_none() || in_media
    } else {
        in_media
    };
    if !shown {
        return;
    }
    let size = u64_of(dev, "size").unwrap_or(0);
    let label = str_of(dev, "label");
    let model = [str_of(disk, "vendor"), str_of(disk, "model")]
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let name = if !label.is_empty() {
        label.to_string()
    } else {
        // Unnamed drives go by their size; the icon tells a stick from a disk image.
        human_size(size)
    };
    let path = str_of(dev, "path").to_string();
    out.push(Device {
        name,
        kind: if loop_dev {
            Kind::Image
        } else if usb {
            Kind::Usb
        } else {
            Kind::Drive
        },
        mount: mount.map(PathBuf::from),
        disk: Some(str_of(disk, "path").to_string())
            .filter(|d| !d.is_empty() && *d != path || loop_dev),
        block: Some(path),
        fstype: fstype.to_string(),
        // The filesystem's own size when mounted, which is what free space is measured against.
        size: u64_of(dev, "fssize").filter(|&s| s > 0).unwrap_or(size),
        used: u64_of(dev, "fsused"),
        detail: model,
        removable: plugged || loop_dev,
    });
}

/// Reads the current drives and shares. Runs lsblk, so call it off the UI thread.
pub fn scan() -> Vec<Device> {
    let lsblk = Command::new("lsblk")
        .args([
            "-J",
            "-b",
            "-o",
            "PATH,PKNAME,TYPE,SIZE,FSTYPE,FSSIZE,FSUSED,LABEL,MOUNTPOINTS,RM,HOTPLUG,TRAN,MODEL,VENDOR",
        ])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    parse(&lsblk, &read("/proc/mounts"), &read("/etc/fstab"))
}

/// Runs udisksctl, turning its errors into a sentence.
fn udisks(args: &[&str]) -> Result<String, String> {
    let out = Command::new("udisksctl")
        .args(args)
        .arg("--no-user-interaction")
        .output()
        .map_err(|e| format!("udisksctl: {e}"))?;
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).trim().to_string());
    }
    Err(explain(&String::from_utf8_lossy(&out.stderr)))
}

/// UDisks' errors are D-Bus noise; say what they mean.
pub fn explain(stderr: &str) -> String {
    if stderr.contains("DeviceBusy") || stderr.contains("target is busy") {
        return tr("It's in use by another program. Close what's using it and try again.");
    }
    if stderr.contains("NotAuthorized") {
        return tr("Not allowed: the password wasn't given.");
    }
    // The useful part comes after the last D-Bus error name.
    let line = stderr
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    line.rsplit_once(": ")
        .map_or(line, |(_, msg)| msg)
        .to_string()
}

/// Mounts a drive and returns where.
pub fn mount(block: &str) -> Result<PathBuf, String> {
    // "Mounted /dev/sda1 at /run/media/me/STICK"
    let out = udisks(&["mount", "-b", block])?;
    out.split_once(" at ")
        .map(|(_, at)| PathBuf::from(at.trim_end_matches('.')))
        .ok_or(out)
}

pub fn unmount(block: &str) -> Result<(), String> {
    udisks(&["unmount", "-b", block]).map(|_| ())
}

/// Unmounts, then powers the drive off (or detaches a disk image), so it can be unplugged.
pub fn eject(d: &Device) -> Result<(), String> {
    let block = d.block.as_deref().ok_or_else(|| tr("Nothing to eject"))?;
    if d.mount.is_some() {
        unmount(block)?;
    }
    match (d.kind, d.disk.as_deref()) {
        (Kind::Image, _) => {
            udisks(&["loop-delete", "-b", d.disk.as_deref().unwrap_or(block)]).map(|_| ())
        }
        // Other partitions on the same stick may still be mounted; then it stays powered.
        (_, Some(disk)) => udisks(&["power-off", "-b", disk]).map(|_| ()),
        (_, None) => udisks(&["power-off", "-b", block]).map(|_| ()),
    }
}

/// Filesystems Ferry offers when formatting, if their mkfs is installed: (UDisks name, what for).
pub fn formats() -> Vec<(&'static str, String)> {
    [
        (
            "exfat",
            "mkfs.exfat",
            tr("exFAT: works on Windows, macOS and Linux"),
        ),
        (
            "vfat",
            "mkfs.vfat",
            tr("FAT32: for old devices; files up to 4 GB"),
        ),
        ("ext4", "mkfs.ext4", tr("ext4: Linux only")),
        ("ntfs", "mkfs.ntfs", tr("NTFS: for Windows")),
    ]
    .into_iter()
    .filter(|(_, tool, _)| {
        Path::new("/usr/bin").join(tool).exists() || Path::new("/usr/sbin").join(tool).exists()
    })
    .map(|(fs, _, what)| (fs, what))
    .collect()
}

/// Erases `block` with a new `fstype` filesystem named `label`, then mounts it.
pub fn format(block: &str, fstype: &str, label: &str) -> Result<PathBuf, String> {
    let name = block.trim_start_matches("/dev/");
    let object = format!(
        "/org/freedesktop/UDisks2/block_devices/{}",
        name.replace('-', "_2d")
    );
    let label = label.replace(['\'', '\\'], "");
    let options =
        format!("{{'label': <'{label}'>, 'update-partition-type': <true>, 'tear-down': <true>}}");
    let out = Command::new("gdbus")
        .args([
            "call",
            "--system",
            "--timeout",
            "3600",
            "--dest",
            "org.freedesktop.UDisks2",
            "--object-path",
        ])
        .arg(&object)
        .args([
            "--method",
            "org.freedesktop.UDisks2.Block.Format",
            fstype,
            &options,
        ])
        .output()
        .map_err(|e| format!("gdbus: {e}"))?;
    if !out.status.success() {
        return Err(explain(&String::from_utf8_lossy(&out.stderr)));
    }
    mount(block)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LSBLK: &str = r#"{"blockdevices": [
      {"path":"/dev/sda","type":"disk","size":1000204886016,"fstype":null,"label":null,"mountpoints":[],
       "rm":false,"hotplug":true,"tran":"usb","model":"X10 Pro","vendor":"Crucial ",
       "children":[{"path":"/dev/sda1","type":"part","size":1000203820544,"fstype":"ext4","fssize":983350071296,
         "fsused":438355267584,"label":null,"mountpoints":["/run/media/me/a3cc"],"rm":false,"hotplug":true,"tran":null}]},
      {"path":"/dev/sdb","type":"disk","size":16000000000,"fstype":"vfat","label":"STICK","mountpoints":[],
       "rm":true,"hotplug":true,"tran":"usb","model":"Cruzer","vendor":"SanDisk"},
      {"path":"/dev/nvme0n1","type":"disk","size":1000000000000,"rm":false,"hotplug":false,"tran":"nvme",
       "children":[{"path":"/dev/nvme0n1p1","type":"part","size":2000000000,"fstype":"vfat","mountpoints":["/boot"]},
                   {"path":"/dev/nvme0n1p2","type":"part","size":998000000000,"fstype":"crypto_LUKS","mountpoints":[]}]},
      {"path":"/dev/loop0","type":"loop","size":67108864,"fstype":"exfat","label":null,"mountpoints":[]},
      {"path":"/dev/loop1","type":"loop","size":67108864,"fstype":"squashfs","mountpoints":["/var/lib/snapd/snap/core/1"]}
    ]}"#;

    const MOUNTS: &str = "systemd-1 /mnt/nas autofs rw 0 0\n\
        192.168.0.15:/media/Data /mnt/nas nfs4 rw 0 0\n\
        /dev/sda1 /run/media/me/a3cc ext4 rw 0 0\n";

    const FSTAB: &str = "# comment\nUUID=6d0b / btrfs rw 0 0\n\
        192.168.0.15:/media/Data /mnt/nas nfs defaults 0 0\n\
        100.124.10.52:/srv/share /mnt/fedora nfs defaults 0 0\n";

    #[test]
    fn tells_drives_from_network_shares() {
        let d = parse(LSBLK, MOUNTS, FSTAB);
        let names: Vec<(&str, Kind)> = d.iter().map(|d| (d.name.as_str(), d.kind)).collect();
        assert_eq!(
            names,
            [
                ("931.5 GiB", Kind::Usb),
                ("STICK", Kind::Usb),
                ("64.0 MiB", Kind::Image),
                ("nas", Kind::Network),
                ("fedora", Kind::Network),
            ]
        );
        // The mounted share keeps its live address; the sleeping one comes from fstab.
        assert_eq!(d[3].detail, "192.168.0.15:/media/Data");
        assert_eq!(d[4].mount, Some(PathBuf::from("/mnt/fedora")));
    }

    #[test]
    fn knows_capacity_and_what_to_power_off() {
        let d = parse(LSBLK, MOUNTS, FSTAB);
        let ssd = &d[0];
        assert_eq!(
            (ssd.block.as_deref(), ssd.disk.as_deref()),
            (Some("/dev/sda1"), Some("/dev/sda"))
        );
        assert_eq!(ssd.detail, "Crucial X10 Pro");
        assert!((ssd.fill().unwrap() - 0.4458).abs() < 0.001);
        assert!(ssd.removable && ssd.summary().contains("free of"));
        // A stick formatted without partitions is its own disk; it isn't mounted yet.
        let stick = &d[1];
        assert_eq!((stick.disk.as_deref(), stick.mount.as_ref()), (None, None));
        assert!(stick.summary().contains("Not mounted"));
    }

    #[test]
    fn reads_lsblk_flat_output_too() {
        // What this machine's lsblk prints: no nesting, the partition's disk only in PKNAME.
        let flat = r#"{"blockdevices": [
          {"path":"/dev/sda","pkname":null,"type":"disk","size":1000204886016,"fstype":null,"mountpoints":[],
           "hotplug":true,"tran":"usb","model":"CT1000X10PROSSD9","vendor":"Micron  "},
          {"path":"/dev/sda1","pkname":"sda","type":"part","size":1000203820544,"fstype":"ext4",
           "fssize":983350071296,"fsused":438355267584,"mountpoints":["/run/media/me/a3cc"],"hotplug":true,"tran":null},
          {"path":"/dev/mapper/root","pkname":"nvme0n1p2","type":"crypt","fstype":"btrfs","mountpoints":["/home","/"]},
          {"path":"/dev/nvme0n1p1","pkname":"nvme0n1","type":"part","fstype":"vfat","mountpoints":["/boot"],"tran":"nvme"}
        ]}"#;
        let d = parse(flat, "", "");
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(
            (d[0].kind, d[0].disk.as_deref()),
            (Kind::Usb, Some("/dev/sda"))
        );
        assert_eq!(d[0].detail, "Micron CT1000X10PROSSD9");
    }

    #[test]
    fn explains_udisks_errors() {
        let busy = "Error unmounting /dev/sda1: GDBus.Error:org.freedesktop.UDisks2.Error.DeviceBusy: Error unmounting /dev/sda1: target is busy\n";
        assert!(explain(busy).contains("in use"));
        let other = "Error mounting /dev/sdb1: GDBus.Error:org.freedesktop.UDisks2.Error.Failed: Wrong fs type\n";
        assert_eq!(explain(other), "Wrong fs type");
    }

    /// Formats, mounts and ejects a real disk image through UDisks. Only ever point it at a
    /// throwaway loop device: `FERRY_LOOP=/dev/loopN cargo test loop_device -- --ignored`.
    #[test]
    #[ignore]
    fn loop_device_end_to_end() {
        let Ok(block) = std::env::var("FERRY_LOOP") else {
            return;
        };
        assert!(block.starts_with("/dev/loop"), "only loop devices");
        let mounted = format(&block, "exfat", "FERRYTEST").expect("formats and mounts");
        assert!(mounted.ends_with("FERRYTEST"), "{mounted:?}");
        std::fs::write(mounted.join("hello.txt"), "hi").unwrap();
        let found = scan()
            .into_iter()
            .find(|d| d.block.as_deref() == Some(block.as_str()))
            .expect("listed");
        assert_eq!(
            (found.name.as_str(), found.kind, found.fstype.as_str()),
            ("FERRYTEST", Kind::Image, "exfat")
        );
        assert!(found.used.is_some() && found.mount.is_some());
        eject(&found).expect("ejects");
        assert!(
            !scan()
                .iter()
                .any(|d| d.block.as_deref() == Some(block.as_str())),
            "gone after eject"
        );
    }
}
