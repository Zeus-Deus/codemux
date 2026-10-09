import { create } from "zustand";

/** Whether Settings → Devices should show its "Add device" dialog. Lets the
 *  composer's device picker deep-link straight into adding a device. */
interface AddDeviceDialogStore {
  open: boolean;
  setOpen: (open: boolean) => void;
}

export const useAddDeviceDialogStore = create<AddDeviceDialogStore>((set) => ({
  open: false,
  setOpen: (open) => set({ open }),
}));
