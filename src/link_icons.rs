pub(crate) const FALLBACK_ICON: &str = "󰌹";
pub(crate) const FILE_ICON: &str = "󰈔";
pub(crate) const CONTENT_ICON: &str = "󰄡";

const HOST_ICONS: &[(&str, &str)] = &[
    ("drive.google.com", "󰊶"),
    ("docs.google.com", "󰊶"),
    ("sheets.google.com", "󰊶"),
    ("maps.google.com", "󰗵"),
    ("maps.google.co.uk", "󰗵"),
    ("play.google.com", "󰊼"),
    ("classroom.google.com", "󰋀"),
    ("keep.google.com", "󰛜"),
    ("translate.google.com", "󰊿"),
    ("analytics.google.com", "󰟌"),
    ("cloud.google.com", ""),
    ("console.cloud.google.com", ""),
    ("google.com", ""),
    ("google.co.uk", ""),
    ("office.com", ""),
    ("microsoft365.com", ""),
    ("microsoft.com", ""),
    ("outlook.com", "󰴢"),
    ("outlook.live.com", "󰴢"),
    ("teams.microsoft.com", "󰊻"),
    ("onedrive.live.com", "󰏊"),
    ("sharepoint.com", "󱎑"),
    ("azure.com", "󰠅"),
    ("portal.azure.com", "󰠅"),
    ("dev.azure.com", "󰿕"),
    ("icloud.com", "󰀸"),
    ("apple.com", ""),
    ("aws.amazon.com", ""),
    ("console.aws.amazon.com", ""),
    ("amazon.com", ""),
    ("amazon.co.uk", ""),
    ("amazonpay.com", ""),
    ("atlassian.net", ""),
    ("jira.com", ""),
    ("confluence.com", ""),
    ("trello.com", ""),
    ("bitbucket.org", ""),
    ("github.com", ""),
    ("github.dev", ""),
    ("github.io", ""),
    ("gitlab.com", ""),
    ("gitpod.io", ""),
    ("gitbook.io", ""),
    ("stackoverflow.com", ""),
    ("stackexchange.com", ""),
    ("npmjs.com", ""),
    ("pypi.org", ""),
    ("packagist.org", ""),
    ("nuget.org", ""),
    ("postman.com", ""),
    ("insomnia.rest", ""),
    ("readthedocs.io", ""),
    ("swagger.io", ""),
    ("slack.com", ""),
    ("discord.com", ""),
    ("discord.gg", ""),
    ("whatsapp.com", "󰖣"),
    ("web.whatsapp.com", "󰖣"),
    ("telegram.org", ""),
    ("t.me", ""),
    ("skype.com", ""),
    ("notion.so", ""),
    ("notion.site", ""),
    ("figma.com", ""),
    ("canva.com", ""),
    ("dropbox.com", ""),
    ("airbnb.com", ""),
    ("uber.com", ""),
    ("cloudflare.com", ""),
    ("workers.dev", ""),
    ("digitalocean.com", ""),
    ("heroku.com", ""),
    ("herokuapp.com", ""),
    ("vercel.com", ""),
    ("vercel.app", ""),
    ("netlify.com", ""),
    ("netlify.app", ""),
    ("firebase.google.com", ""),
    ("firebaseapp.com", ""),
    ("supabase.com", ""),
    ("appwrite.io", ""),
    ("railway.app", ""),
    ("openstack.org", ""),
    ("portainer.io", ""),
    ("rancher.com", ""),
    ("kubernetes.io", ""),
    ("terraform.io", ""),
    ("app.terraform.io", ""),
    ("vaultproject.io", ""),
    ("argoproj.github.io", ""),
    ("jenkins.io", ""),
    ("circleci.com", ""),
    ("travis-ci.com", ""),
    ("grafana.com", ""),
    ("prometheus.io", ""),
    ("splunk.com", ""),
    ("sentry.io", ""),
    ("elastic.co", ""),
    ("kibana.com", ""),
    ("sonarsource.com", ""),
    ("sonarcloud.io", ""),
    ("mongodb.com", ""),
    ("redis.io", ""),
    ("redis.com", ""),
    ("neo4j.com", ""),
    ("postgresql.org", ""),
    ("mysql.com", ""),
    ("mariadb.org", ""),
    ("couchbase.com", ""),
    ("couchdb.apache.org", ""),
    ("influxdata.com", ""),
    ("oracle.com", ""),
    ("facebook.com", ""),
    ("instagram.com", ""),
    ("twitter.com", ""),
    ("x.com", ""),
    ("linkedin.com", ""),
    ("reddit.com", ""),
    ("wikipedia.org", ""),
    ("pinterest.com", ""),
    ("snapchat.com", ""),
    ("mastodon.social", ""),
    ("quora.com", ""),
    ("medium.com", ""),
    ("dribbble.com", ""),
    ("behance.net", ""),
    ("tumblr.com", ""),
    ("flickr.com", ""),
    ("yahoo.com", ""),
    ("yelp.com", ""),
    ("goodreads.com", ""),
    ("youtube.com", ""),
    ("youtu.be", ""),
    ("twitch.tv", ""),
    ("spotify.com", ""),
    ("soundcloud.com", ""),
    ("vimeo.com", ""),
    ("steampowered.com", ""),
    ("steamcommunity.com", ""),
    ("imdb.com", ""),
    ("kickstarter.com", ""),
    ("patreon.com", ""),
    ("ebay.com", ""),
    ("etsy.com", ""),
    ("paypal.com", ""),
    ("stripe.com", ""),
    ("woocommerce.com", ""),
    ("shopware.com", ""),
    ("magento.com", ""),
    ("wordpress.com", ""),
    ("wordpress.org", ""),
    ("drupal.org", ""),
    ("moodle.org", ""),
    ("webflow.com", ""),
    ("storybook.js.org", ""),
];

const PROTOCOL_ICONS: &[(&str, &str)] = &[
    ("mailto", ""),
    ("tel", ""),
    ("ssh", "󰣀"),
    ("tg", ""),
    ("whatsapp", "󰖣"),
    ("skype", ""),
    ("steam", ""),
    ("rss", "󰑫"),
    ("magnet", "󰍇"),
    ("http", "󰖟"),
    ("https", "󰖟"),
];

pub(crate) fn icon(value: &str) -> &'static str {
    let normalized = value.trim().to_ascii_lowercase();
    let protocol = protocol(&normalized);
    match protocol {
        Some("file") => return FILE_ICON,
        Some("content") => return CONTENT_ICON,
        _ => {}
    }

    if let Some(host) = host(&normalized) {
        if domain_matches(host, "atlassian.net") && normalized.contains("/wiki") {
            return "";
        }
        if let Some((_, icon)) = HOST_ICONS
            .iter()
            .filter(|(domain, _)| domain_matches(host, domain))
            .max_by_key(|(domain, _)| domain.len())
        {
            return icon;
        }
    }

    protocol
        .and_then(|protocol| {
            PROTOCOL_ICONS
                .iter()
                .find_map(|(candidate, icon)| (*candidate == protocol).then_some(*icon))
        })
        .unwrap_or(FALLBACK_ICON)
}

fn protocol(value: &str) -> Option<&str> {
    let separator = value.find(':')?;
    let candidate = &value[..separator];
    (!candidate.is_empty()).then_some(candidate)
}

fn host(value: &str) -> Option<&str> {
    let authority = if value.starts_with("www.") {
        value
    } else {
        value.split_once("://")?.1
    };
    let authority = authority.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    Some(authority.split(':').next().unwrap_or(authority))
}

fn domain_matches(host: &str, domain: &str) -> bool {
    host == domain
        || host
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_hosts_and_www_urls_to_brand_icons() {
        assert_eq!(icon("https://github.com/daure/tuido"), "");
        assert_eq!(icon("www.google.com/search?q=tuido"), "");
        assert_eq!(icon("https://www.airbnb.com/rooms/1"), "");
        assert_eq!(icon("https://team.atlassian.net/browse/ABC-1"), "");
        assert_eq!(icon("https://team.atlassian.net/wiki/spaces/ABC"), "");
        assert_eq!(icon("https://firebase.google.com/docs"), "");
        assert_eq!(icon("https://teams.microsoft.com/meeting"), "󰊻");
        assert_eq!(icon("https://argoproj.github.io/cd"), "");
    }

    #[test]
    fn host_matching_requires_a_domain_boundary() {
        assert_eq!(icon("https://notgithub.com/item"), "󰖟");
        assert_eq!(icon("https://github.com.evil.example/item"), "󰖟");
    }

    #[test]
    fn maps_protocols_and_fallbacks() {
        assert_eq!(icon("file:///tmp/report.pdf"), FILE_ICON);
        assert_eq!(icon("content://media/1"), CONTENT_ICON);
        assert_eq!(icon("mailto:someone@example.com"), "");
        assert_eq!(icon("custom+app://item/42"), FALLBACK_ICON);
        assert_eq!(icon("https://example.com"), "󰖟");
    }
}
