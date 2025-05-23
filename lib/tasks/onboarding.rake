namespace :onboarding do
  desc 'Reset onboarding status for all users'
  task reset_all: :environment do
    User.update_all(onboarding_completed: false)
    puts 'Reset onboarding status for all users.'
  end

  desc 'Mark onboarding as completed for all users'
  task complete_all: :environment do
    User.update_all(onboarding_completed: true)
    puts 'Marked onboarding as completed for all users.'
  end

  desc 'Reset onboarding status for specific user by email'
  task :reset_user, [:email] => :environment do |_t, args|
    if args[:email].present?
      user = User.find_by(email: args[:email])
      if user
        user.update(onboarding_completed: false)
        puts "Reset onboarding status for user: #{args[:email]}"
      else
        puts "User not found with email: #{args[:email]}"
      end
    else
      puts 'Please provide an email address: rake onboarding:reset_user[user@example.com]'
    end
  end
end
