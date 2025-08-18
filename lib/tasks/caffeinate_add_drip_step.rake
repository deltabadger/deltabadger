desc 'rake task to add a caffeinate drip step to an existing campaign'
task caffeinate_add_drip_step: :environment do
  # Adding a new drip step will send this step to anyone who was not unsubscribed, even if
  # they ended subscription.
  # If the new drip step should have been sent before current time, it will be sent right away
  # (after executing Caffeinate.perform!).

  campaign = 'onboarding'
  new_drip_step = 'new_action' # new drip step name
  new_drip_step_delay = 1.day
  after_drip_step = 'welcome_to_my_cool_app' # previous drip step name

  campaign = Caffeinate::Campaign.find_by!(slug: campaign)
  campaign.subscriptions.ended.update_all(ended_at: nil)

  campaign.subscriptions.active.find_each do |subscription|
    new_drip_added = false
    subscription.mailings.order(:send_at).each do |mailing|
      if mailing.mailer_action == after_drip_step
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
  end
end
