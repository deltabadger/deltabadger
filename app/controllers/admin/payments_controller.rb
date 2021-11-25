module Admin
  class PaymentsController < Admin::ApplicationController
    # Overwrite any of the RESTful controller actions to implement custom behavior
    # For example, you may want to send an email after a foo is updated.

    # Override this method to specify custom lookup behavior.
    # This will be used to set the resource for the `show`, `edit`, and `update`
    # actions.
    #
    # def find_resource(param)
    #   Foo.find_by!(slug: param)
    # end

    # Override this if you have certain roles that require a subset
    # this will be used to set the records shown on the `index` action.
    #
    # def scoped_resource
    #  if current_user.super_admin?
    #    resource_class
    #  else
    #    resource_class.with_less_stuff
    #  end
    # end

    # See https://administrate-prototype.herokuapp.com/customizing_controller_actions
    # for more information
    PAID_STATUSES = %w[paid confirmed complete].freeze

    def model_name
      :payment
    end

    def update
      payment = Payment.find(params[:id])
      status_before_update = payment.status
      super

      payment.reload
      return unless grant_commission?(status_before_update, payment.status)

      Affiliates::GrantCommission.new.call(referee: payment.user, payment: payment)
    end

    def csv
      from = params[:from]
      to = params[:to]
      file = Admin::GeneratePaymentsCsv.call(from: from, to: to)
      send_data(file, filename: csv_filename(from, to))
    end

    def confirm
      payment = Payment.find(params[:id])
      payment.update(status: 2, paid_at: payment['created_at'])
      redirect_back(fallback_location: admin_payments_path)
    end

    private

    def csv_filename(from, to)
      "deltabadger-payments-#{Time.now.strftime('%F')}#{date_range(from, to)}.csv"
    end

    def date_range(from, to)
      "#{format_date('from', from)}#{format_date('to', to)}"
    end

    def format_date(rel, date)
      date.blank? ? '' : "-#{rel}-#{Date.parse(date)}"
    end

    def grant_commission?(old_status, new_status)
      old_status != new_status && new_status.in?(PAID_STATUSES)
    end
  end
end
