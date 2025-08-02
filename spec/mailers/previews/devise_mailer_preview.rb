# Preview all emails at http://localhost:3000/rails/mailers/devise_mailer
class DeviseMailerPreview < ActionMailer::Preview
  def confirmation_instructions
    user = User.new(email: 'test@example.com', name: 'Test User')
    CustomDeviseMailer.confirmation_instructions(user, 'faketoken')
  end

  def reset_password_instructions
    user = User.new(email: 'test@example.com', name: 'Test User')
    CustomDeviseMailer.reset_password_instructions(user, 'faketoken')
  end

  def email_changed
    user = User.new(email: 'new@example.com', name: 'Test User')
    CustomDeviseMailer.email_changed(user)
  end

  def confirm_email
    user = User.new(email: 'old@example.com', name: 'Test User', unconfirmed_email: 'new@example.com')
    token = Devise.token_generator.generate(User, :confirmation_token)
    CustomDeviseMailer.confirm_email(user, token)
  end

  def password_change
    user = User.new(email: 'test@example.com', name: 'Test User')
    CustomDeviseMailer.password_change(user)
  end

  def email_already_taken
    # Find or create a user for preview purposes
    email = 'preview@example.com'
    User.find_by(email: email) || User.create!(
      email: email,
      name: 'Preview User',
      password: 'password123',
      confirmed_at: Time.current
    )

    CustomDeviseMailer.email_already_taken(email)
  end
end
