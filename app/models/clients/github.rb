# frozen_string_literal: true

# Reads the repository's latest release, and nothing else. The update notice is cosmetic, so every
# failure — including the transient network errors Client#with_rescue re-raises for trading callers
# that must retry — comes back as nil rather than an exception: there is nothing here worth
# breaking a settings page over.
class Clients::Github < Client
  REPOSITORY = 'deltabadger/deltabadger'
  BASE_URL = 'https://api.github.com'
  RELEASES_URL = "https://github.com/#{REPOSITORY}/releases".freeze
  MANUAL_URL = "https://github.com/#{REPOSITORY}/blob/main/manual".freeze

  # Only a plain three-part version is a release anyone can install. /releases/latest already
  # excludes drafts and prereleases, so this mostly guards against a tag like `nightly` becoming
  # the latest release by hand.
  RELEASE_TAG = /\Av?(\d+\.\d+\.\d+)\z/

  OPTIONS = {
    request: {
      open_timeout: 5,
      read_timeout: 10
    }
  }.freeze

  # Unauthenticated, at most one call per install per day. The User-Agent GitHub requires carries
  # no version — an install that opted out of nothing still owes GitHub no fingerprint.
  HEADERS = {
    'Accept' => 'application/vnd.github+json',
    'User-Agent' => 'Deltabadger'
  }.freeze

  def latest_release
    response = connection.get("repos/#{REPOSITORY}/releases/latest")
    body = response.body
    return nil unless body.is_a?(Hash)

    tag = body['tag_name'].to_s
    version = RELEASE_TAG.match(tag)&.captures&.first
    return nil if version.nil?

    # Built from the tag this just validated, not from the payload's html_url: the response is
    # remote input that ends up in a link on a settings page, and a tag matching RELEASE_TAG is
    # URL-safe by construction.
    { version: version, url: "#{RELEASES_URL}/tag/#{tag}" }
  rescue StandardError => e
    Rails.logger.info("Update check skipped: #{e.class}: #{e.message}")
    nil
  end

  private

  def connection
    @connection ||= Faraday.new(url: BASE_URL, headers: HEADERS, **OPTIONS) do |config|
      config.response :json
      config.response :raise_error
      config.adapter :net_http
    end
  end
end
