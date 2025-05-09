desc 'rake task to update barbell bots transient data'
task update_barbell_bots_transient_data: :environment do
  Bot.where(type: 'Bots::Barbell').find_each do |bot|
    next if bot.transient_data.blank?

    puts "Updating bot #{bot.id}"
    transient_data = bot.transient_data
    if transient_data['last_action_job_at_iso8601'].present?
      puts "setting last_action_job_at to #{transient_data['last_action_job_at_iso8601']}"
      transient_data['last_action_job_at'] = transient_data['last_action_job_at_iso8601']
      puts "deleting last_action_job_at_iso8601, value was: #{transient_data['last_action_job_at_iso8601']}"
      transient_data.delete('last_action_job_at_iso8601')
    end
    if transient_data['last_pending_quote_amount_calculated_at'].present?
      # puts "setting settings_changed_at to #{transient_data['last_pending_quote_amount_calculated_at']} value was: #{bot.settings_changed_at}"
      # bot.update!(settings_changed_at: transient_data['last_pending_quote_amount_calculated_at'])
      puts "deleting last_pending_quote_amount_calculated_at, value was: #{transient_data['last_pending_quote_amount_calculated_at']}"
      transient_data.delete('last_pending_quote_amount_calculated_at')
    end
    if transient_data['pending_quote_amount'].present?
      # puts "setting missed_quote_amount to #{transient_data['pending_quote_amount']}"
      # transient_data['missed_quote_amount'] = transient_data['pending_quote_amount']
      puts "deleting pending_quote_amount, value was: #{transient_data['pending_quote_amount']}"
      transient_data.delete('pending_quote_amount')
    end
    bot.update!(transient_data: transient_data.compact)
  end
end
