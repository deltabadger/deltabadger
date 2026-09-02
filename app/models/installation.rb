# frozen_string_literal: true

# How this copy of Deltabadger was installed, which is the same question as how it gets updated.
#
# The answer is declared, never guessed. Every signal available from inside the process proves
# something narrower than what it would be used to claim: /.dockerenv proves "a container", not
# "started with Compose", and MARKET_DATA_URL proves "a market-data service was configured", which
# the manual documents for self-hosted installs pointing at their own. Both ways of guessing wrong
# cost the user something real — an install wrongly read as self-updating never hears about an
# update, and one wrongly read as Compose is handed a command that cannot update it — so an install
# that has not been told what it is stays :unknown and gets pointed at the manual, which covers
# every way of starting it.
#
# The marker is set by each launcher: src-tauri/src/lib.rs, docker-compose.yml and
# deltabadger/docker-compose.yml. An install somebody else keeps up to date needs no marker of its
# own — it switches the check off with DELTABADGER_UPDATE_CHECK=false and this section stays away.
class Installation
  MARKER = 'DELTABADGER_PLATFORM'

  PLATFORMS = %i[desktop umbrel docker].freeze

  def self.platform
    platform_from_env(ENV)
  end

  def self.platform_from_env(env)
    marker = env[MARKER].to_s.strip.downcase
    return :unknown if marker.empty?

    platform = marker.to_sym
    return platform if PLATFORMS.include?(platform)

    Rails.logger.warn("Unrecognised #{MARKER}=#{marker.inspect}; " \
                      "expected one of #{PLATFORMS.join(', ')}. Treating this install as unknown.")
    :unknown
  end

  # The one platform that installs updates itself: Tauri's signed updater prompts on launch, and
  # its manifest rather than the GitHub release decides what is installable. So the desktop build
  # gets a button instead of instructions, and never asks GitHub for a release of its own.
  def self.desktop?(platform = self.platform)
    platform == :desktop
  end
end
