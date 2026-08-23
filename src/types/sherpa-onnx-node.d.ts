declare module "sherpa-onnx-node" {
  export interface Waveform {
    sampleRate: number;
    samples: Float32Array;
  }

  export interface SpeechSegment {
    start: number;
    samples: Float32Array;
  }

  export class Vad {
    constructor(config: object, bufferSizeInSeconds: number);
    acceptWaveform(samples: Float32Array): void;
    isEmpty(): boolean;
    isDetected(): boolean;
    pop(): void;
    clear(): void;
    front(enableExternalBuffer?: boolean): SpeechSegment;
    reset(): void;
    flush(): void;
  }

  export interface SpeakerStream {
    acceptWaveform(waveform: Waveform): void;
  }

  export class SpeakerEmbeddingExtractor {
    readonly dim: number;
    constructor(config: object);
    createStream(): SpeakerStream;
    isReady(stream: SpeakerStream): boolean;
    compute(stream: SpeakerStream, enableExternalBuffer?: boolean): Float32Array;
  }

  export interface OfflineStream {
    acceptWaveform(waveform: Waveform): void;
  }

  export interface OfflineRecognizerResult {
    text: string;
    tokens?: string[];
    timestamps?: number[];
  }

  export class OfflineRecognizer {
    constructor(config: object);
    createStream(hotwords?: string): OfflineStream;
    decode(stream: OfflineStream): void;
    decodeAsync(stream: OfflineStream): Promise<OfflineRecognizerResult>;
    getResult(stream: OfflineStream): OfflineRecognizerResult;
  }

  const sherpa: {
    Vad: typeof Vad;
    SpeakerEmbeddingExtractor: typeof SpeakerEmbeddingExtractor;
    OfflineRecognizer: typeof OfflineRecognizer;
  };
  export default sherpa;
}
