declare global {
  namespace App {
    interface Locals {
      session?: {
        authenticated: boolean;
      };
    }
    interface PageData {
      session?: App.Locals['session'];
    }
  }
}

export {};
