class NextResultFetchingAt < BaseService
  def initialize(
    calculate_restart_delay: CalculateFetchingRestartDelay.new
  )
    @calculate_restart_delay = calculate_restart_delay
  end

  def call(bot)
    delay = if bot.fetch_restarts.zero?
              normal_delay
            else
              restart_delay(bot)
            end

    delay.since(Time.now)
  end

  private

  def restart_delay(bot)
    @calculate_restart_delay.call(bot.fetch_restarts)
  end

  def normal_delay
    3.seconds
  end
end
