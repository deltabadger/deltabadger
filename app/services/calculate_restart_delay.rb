class CalculateRestartDelay
  BASE_DELAY = 15.minutes.freeze

  def call(bot)
    multiplier = 2**(bot.restarts - 1)
    BASE_DELAY * multiplier
  end
end
