module DeviseHelpers
  def sign_in_user
    user = User.create(
      email: 'test@test.com',
      password: 'password',
      password_confirmation: 'password',
      confirmed_at: Time.now
    )
    sign_in user

    user
  end
end
