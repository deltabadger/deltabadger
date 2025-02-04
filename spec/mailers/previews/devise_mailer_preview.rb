class DeviseMailerPreview < ActionMailer::Preview
  def confirmation_instructions
    user = User.new(
      email: 'test@example.com',
      name: 'Mathias'
    )
    token = Devise.token_generator.generate(User, :confirmation_token)
    user.instance_variable_set(:@raw_confirmation_token, token)
    CustomDeviseMailer.confirmation_instructions(user, token)
  end

  def reset_password_instructions
    user = User.new(
      email: 'test@example.com',
      name: 'Mathias'
    )
    token = Devise.token_generator.generate(User, :reset_password_token)
    user.instance_variable_set(:@raw_reset_password_token, token)
    CustomDeviseMailer.reset_password_instructions(user, token)
  end

  def password_change
    user = User.new(
      email: 'test@example.com',
      name: 'Mathias'
    )
    CustomDeviseMailer.password_change(user)
  end

  def email_changed
    user = User.new(
      email: 'test@example.com',
      name: 'Mathias',
      unconfirmed_email: 'new@example.com'
    )
    CustomDeviseMailer.email_changed(user)
  end

  def email_already_taken
    # Create a Mathias first to ensure it exists in the preview
    test_email = 'test@example.com'
    user = User.find_by(email: test_email)
    unless user
      user = User.new(
        email: test_email,
        name: 'Mathias',
        password: 'password123',
        password_confirmation: 'password123'
      )
      user.save(validate: false)
    end

    CustomDeviseMailer.email_already_taken(test_email)
  end
end
