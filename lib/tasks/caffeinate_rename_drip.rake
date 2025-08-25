desc 'rake task to rename a caffeinate drip'
task caffeinate_rename_drip: :environment do
  mailer = 'OnboardingMailer'
  from_drip_step = 'reminder' # drip name
  to_drip_step = 'suggested' # new drip name

  puts "Updating #{mailer} drip step #{from_drip_step} to #{to_drip_step}"
  Caffeinate::Mailing.upcoming
                     .where(mailer_class: mailer, mailer_action: from_drip_step)
                     .update_all(mailer_action: to_drip_step)
end
