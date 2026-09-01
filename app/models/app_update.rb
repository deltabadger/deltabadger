# frozen_string_literal: true

# Whether a newer Deltabadger exists, for installs that have to be updated by their owner.
#
# Reading is free: the widget only ever reads the cache, so no page render waits on GitHub and no
# cold cache can send every Puma worker out at once. The one call lives in AppUpdate::CheckJob,
# twice a day.
class AppUpdate
  CACHE_KEY = 'app_update/latest_release/v1'

  # An hour longer than the twelve-hour schedule, so an answer survives one missed run.
  CACHE_TTL = 25.hours

  OPT_OUT = 'DELTABADGER_UPDATE_CHECK'

  # Shown only where the marker says Compose, because it is the only place it works.
  COMPOSE_COMMAND = 'docker compose pull && docker compose up -d'

  def self.current_version
    Rails.application.config.version
  end

  # False where something else delivers updates, and wherever the owner has switched the check off.
  # This is the only outbound call an otherwise offline install makes, so it owes them a switch —
  # read through the application's own resolver because ActiveModel::Type::Boolean has a closed
  # false list that reads 'no' as true.
  def self.enabled?
    return false if Installation.managed_updates?

    Deltabadger::Application.env_boolean(ENV[OPT_OUT]) != false
  end

  # Writes only a success. A failed run leaves the last good answer to expire on its own rather
  # than blanking a notice the user has already been shown.
  def self.refresh!
    return nil unless enabled?

    release = Clients::Github.new.latest_release
    return nil if release.nil?

    Rails.cache.write(CACHE_KEY, release, expires_in: CACHE_TTL)
    release
  end

  def self.latest
    Rails.cache.read(CACHE_KEY)
  end

  def self.latest_version
    latest&.fetch(:version, nil)
  end

  def self.latest_url
    latest&.fetch(:url, nil)
  end

  def self.available?
    version = latest_version
    return false if version.blank?

    Gem::Version.new(version) > Gem::Version.new(current_version)
  rescue ArgumentError
    false
  end
end
