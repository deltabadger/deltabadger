desc 'rake task to schedule a one time mailing'
task caffeinate_schedule_one_time_mailing: :environment do
  # Adding a new drip step will send this step to anyone who was not unsubscribed, even if they ended subscription.
  #
  # Steps to send a 1-time email from a campaign:
  #
  # Setup
  # 1. Create a unique drip step, e.g. Drippers::ProductUpdates -> drip :first_email
  # * make the drip unique so stats are not mixed with other drips in the campaign
  # 2. Create the mailer action, e.g. ProductUpdatesMailer#first_email
  # 3. Edit the rake task caffeinate_schedule_one_time_mailing to use the new drip step
  # 4. Create the email template, locales etc
  #
  # Sending
  # 5. Run the rake task:  rake caffeinate_schedule_one_time_mailing
  #
  # Cleanup
  # 6. Comment the sent drip in the dripper, e.g. Drippers::ProductUpdates -> # drip :first_email
  #
  # * mailer actions and locale keys can be deleted after sending, the only place where we need to keep track of drip names is in the dripper

  campaign_slug = 'product_updates'
  mailer_class = 'ProductUpdatesMailer'
  drip_step = 'fireheads_restart' # new drip step name

  # campaign_slug = 'newsletter'
  # mailer_class = 'NewsletterMailer'
  # drip_step = 'first_email' # new drip step name

  campaign = Caffeinate::Campaign.find_by!(slug: campaign_slug)

  # normal send
  campaign.subscriptions.subscribed.find_each do |subscription|
    next if subscription.mailings.find_by(mailer_action: drip_step).present?

    puts "Adding new drip step #{drip_step} for #{subscription.subscriber.email}"
    subscription.update!(ended_at: nil) if subscription.ended?
    subscription.mailings.create!(send_at: subscription.created_at,
                                  mailer_class: mailer_class,
                                  mailer_action: drip_step)
  end

  # # send a small amount
  # limit = 10
  # campaign.subscriptions.subscribed.find_each do |subscription|
  #   # next unless subscription.subscriber.bots.working.any?
  #   next if subscription.mailings.find_by(mailer_action: drip_step).present?
  #   break if limit.zero?

  #   limit -= 1
  #   puts "Adding new drip step #{drip_step} for #{subscription.subscriber.email}"
  #   subscription.update!(ended_at: nil) if subscription.ended?
  #   subscription.mailings.create!(send_at: subscription.created_at,
  #                                 mailer_class: mailer_class,
  #                                 mailer_action: drip_step)
  # end

  # # send in batches
  # batch_sizes = [1000, 2000, 4000, 9000]
  # batch = 0
  # batch_day_delay = 1
  # campaign.subscriptions.subscribed.find_each do |subscription|
  #   # next unless subscription.subscriber.bots.working.any?
  #   next if subscription.mailings.find_by(mailer_action: drip_step).present?

  #   batch_sizes[batch] -= 1
  #   batch += 1 if batch_sizes[batch].zero?

  #   send_at = (batch + batch_day_delay).days.from_now
  #   puts "Adding new drip step #{drip_step} for #{subscription.subscriber.email} at #{send_at}"
  #   subscription.update!(ended_at: nil) if subscription.ended?
  #   subscription.mailings.create!(send_at: send_at,
  #                                 mailer_class: mailer_class,
  #                                 mailer_action: drip_step)
  # end
end
