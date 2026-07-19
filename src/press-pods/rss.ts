import { truncate } from "@micthiesen/mitools/strings";
import { escapeXml } from "@micthiesen/mitools/xml";
import { Podcast } from "podcast";
import { getAllEpisodes } from "./persistence.js";
import { prepareTextForRss } from "./rssText.js";

const FEED_EPISODE_LIMIT = 50;

/**
 * Build the podcast RSS feed. `baseUrl` is the public origin the podcast
 * client will fetch enclosures from (no trailing slash).
 */
export function buildPressPodsFeed(baseUrl: string): string {
  const imageUrl = `${baseUrl}/pods/logo.jpeg`;

  const feed = new Podcast({
    title: "PressPods",
    description: "A podcast of the latest news from the web, read aloud by a robot",
    author: "Michael Thiesen",
    siteUrl: "https://github.com/micthiesen/omni-notify",
    language: "en",
    imageUrl,
    itunesImage: imageUrl,
  });

  for (const episode of getAllEpisodes().slice(0, FEED_EPISODE_LIMIT)) {
    const excerpt = episode.excerpt ?? truncate(episode.content, 255);
    // The description is HTML (clients render show notes as HTML); dynamic
    // text is entity-escaped so article-controlled content can't inject tags.
    const safeUrl = escapeXml(episode.articleUrl);
    feed.addItem({
      title: episode.title,
      description:
        `${escapeXml(excerpt)}` +
        `<br><a href="${safeUrl}">${safeUrl}</a>` +
        `<br><br>${prepareTextForRss(episode.content)}`,
      enclosure: {
        url: `${baseUrl}/pods/audio/${episode.audioFile}`,
        type: "audio/mpeg",
        size: episode.fileBytes,
      },
      guid: episode.episodeId,
      author: episode.author,
      date: new Date(episode.createdAt),
      itunesAuthor: episode.author,
      itunesDuration: episode.durationSeconds,
      itunesImage: episode.leadImageUrl,
      itunesSubtitle: excerpt,
    });
  }

  return feed.buildXml();
}

/** Cheap change marker for feed caching. */
export function latestEpisodeId(): string {
  return getAllEpisodes()[0]?.episodeId ?? "no-episodes";
}
