export async function isAndroidNativeApp(): Promise<boolean> {
  const { Capacitor } = await import("@capacitor/core");
  return Capacitor.getPlatform() === "android";
}

export async function leaveAndroidApp(): Promise<boolean> {
  if (!(await isAndroidNativeApp())) return false;
  const { App } = await import("@capacitor/app");
  await App.minimizeApp();
  return true;
}
