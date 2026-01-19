export abstract class Task {
  public abstract name: string;
  public abstract run(): Promise<void>;
}
