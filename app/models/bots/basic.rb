class Bots::Basic < Bot
  include LegacyMethods
  include Schedulable

  def restarting?
    restart_params = GetRestartParams.call(bot_id: id)
    restart_params[:restartType] == 'missed'
  end

  def restarting_within_interval?
    restart_params = GetRestartParams.call(bot_id: id)
    restart_params[:restartType] == 'onSchedule'
  end

  def missed_amount
    restart_params = GetRestartParams.call(bot_id: id)
    restart_params[:missedAmount]
  end

  private

  def action_job_config
    {
      queue: exchange.name.downcase,
      class: 'MakeTransactionWorker',
      args: [id]
    }
  end
end
