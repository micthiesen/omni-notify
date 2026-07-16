export interface PodcastIndexEpisode {
  title: string;
  feedTitle: string;
  feedUrl: string;
  /** Omitted when the feed's iTunes ID is 0/absent. */
  feedItunesId?: number;
  guid: string;
  enclosureUrl: string;
  /** The episode's web page, from RawEpisode.link. */
  episodeUrl?: string;
  /** Epoch ms (RawEpisode.datePublished is epoch seconds). */
  publishedAt: number;
  /** Rounded from RawEpisode.duration (seconds); omitted when duration is absent/0. */
  durationMinutes?: number;
  description: string;
  /** RawEpisode.image, falling back to RawEpisode.feedImage. */
  artworkUrl?: string;
}
