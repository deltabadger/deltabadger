class TestMailer < ApplicationMailer
  def test_email(user)
    @user = user
    set_locale(@user)

    mail(to: @user.email, subject: t('.subject'))
  end
end
