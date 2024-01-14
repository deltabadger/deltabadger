class SubscriptionMailer < ApplicationMailer
  def subscription_granted
    @user = params[:user]
    @subscription_plan = params[:subscription_plan]

    mail(
      to: @user.email,
      subject: I18n.t(
        'subscription_mailer.subscription_granted.subject',
        plan_name: @subscription_plan.display_name
      )
    )
  end

  def after_wire_transfer
    @user = params[:user]
    @subscription_plan = params[:subscription_plan]
    @name = params[:name]
    @type = params[:type]
    @amount = params[:amount]

    mail(
      to: @user.email,
      from: 'jan@deltabadger.com',
      subject: "#{@subscription_plan.display_name} plan granted!"
    ) do |format|
      format.html { render layout: 'plain_mail' }
    end
  end

  def invoice
    @user = params[:user]
    @payment = params[:payment]

    mail(to: @user.email, subject: default_i18n_subject)
  end

  def wire_transfer_summary
    @email = params[:email]
    @subscription_plan = params[:subscription_plan]
    @first_name = params[:first_name]
    @last_name = params[:last_name]
    @country = params[:country]
    @amount = params[:amount]

    id = get_next_id

    mail(
      to: 'jan@deltabadger.com',
      subject: "New wire transfer, ##{id}"
    )
  end

  private

  def get_next_id
    res = ActiveRecord::Base.connection.execute("SELECT nextval('wire_transfer_id_seq')")
    res[0]['nextval']
  end
end
