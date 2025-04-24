class Bots::Withdrawal < Bot
  include LegacyMethods
  include Schedulable

  def restarting?
    false
    # restart_params = GetRestartParams.call(bot_id: id)
    # restart_params[:restartType] == 'missed'
  end

  def restarting_within_interval?
    false
    # restart_params = GetRestartParams.call(bot_id: id)
    # restart_params[:restartType] == 'onSchedule'
  end

  def missed_amount
    0
    # restart_params = GetRestartParams.call(bot_id: id)
    # restart_params[:missedAmount]
  end

  private

  def action_job_config
    {
      queue: exchange.name.downcase,
      class: 'MakeWithdrawalWorker',
      args: [id]
    }
  end
end
