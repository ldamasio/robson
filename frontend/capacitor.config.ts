import type { CapacitorConfig } from "@capacitor/cli";

const config: CapacitorConfig = {
  appId: "br.ia.rbx.robson",
  appName: "Robson",
  webDir: "build",
  loggingBehavior: "debug",
  backgroundColor: "#07080A",
  server: {
    // Keep the native origin inside the existing production CORS allow-list.
    // Capacitor still serves the committed web bundle from the device.
    hostname: "robson.rbx.ia.br",
    androidScheme: "https",
  },
  android: {
    backgroundColor: "#07080A",
    minWebViewVersion: 60,
    webContentsDebuggingEnabled: false,
    allowMixedContent: false,
  },
};

export default config;
