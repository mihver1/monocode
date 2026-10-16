export type BrowserProfile = {
  id: string;
  name: string;
  browser: string;
  executable: string;
  dataDir: string;
  directory: string;
};

export type BrowserStatus = {
  running: boolean;
  selectedProfile: BrowserProfile | null;
  profiles: BrowserProfile[];
  sources: BrowserProfile[];
};
