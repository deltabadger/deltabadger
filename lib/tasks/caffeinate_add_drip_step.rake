desc 'rake task to add a caffeinate drip step to an existing campaign'
task caffeinate_add_drip_step: :environment do
  # Adding a new drip step will send this step to anyone who was not unsubscribed, even if
  # they ended subscription.
  # If the new drip step happens to be sent before current time, it will be sent right away
  # (after executing Caffeinate.perform!).

  # FIXME: instead of this task, better use CampaignSubscription.refuel!(offset: :created_at)

  campaign = 'newsletter'
  mailer_class = 'NewsletterMailer'
  new_drip_step = 'first_email' # new drip step name
  new_drip_step_delay = 1.day
  after_drip_step = nil # previous drip step name, nil if first drip step

  campaign = Caffeinate::Campaign.find_by!(slug: campaign)
  campaign.subscriptions.ended.update_all(ended_at: nil)

  campaign.subscriptions.find_each do |subscription|
    new_drip_added = false
    subscription.mailings.order(:send_at).each do |mailing|
      if after_drip_step.nil? || mailing.mailer_action == after_drip_step
        puts "Adding new drip step #{new_drip_step} at #{mailing.send_at + new_drip_step_delay}"
        subscription.mailings.create!(send_at: mailing.send_at + new_drip_step_delay,
                                      mailer_class: mailing.mailer_class,
                                      mailer_action: new_drip_step)
        new_drip_added = true
      elsif new_drip_added
        puts "Updating old drip step #{mailing.mailer_action} to #{mailing.send_at + new_drip_step_delay}"
        mailing.update!(send_at: mailing.send_at + new_drip_step_delay)
      end
    end
    unless new_drip_added
      # only happens if there are no mailings for this campaign
      puts "Adding new drip step #{new_drip_step} at #{subscription.created_at + new_drip_step_delay}"
      subscription.mailings.create!(send_at: subscription.created_at + new_drip_step_delay,
                                    mailer_class:,
                                    mailer_action: new_drip_step)
    end
  end
end
