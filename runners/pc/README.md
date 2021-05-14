# Solo 2 PC

This is a runner for Solo 2 on PCs (currently tested only on Linux).

The idea is to use the `usbip-device` implementation of a `usb-device`.

To prepare the system:
- `modprobe usbip_host`, which lets the Rust binary act as USB/IP server
- `modprobe vhci_hcd`, which adds a virtual USB hub

After running `cargo run --release`, command `usbip list -r localhost` should
show one device. Using `sudo usbip attach -r localhost -b 1-1`, this device
should then enumerate in the `dmesg` logs.
