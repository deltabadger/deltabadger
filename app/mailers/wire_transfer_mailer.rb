class WireTransferMailer < ApplicationMailer
  def new_wire_transfer
    @params = params[:wire_params]

    mail(to: 'sales@deltabadger.com', subject: 'New wire transfer')
  end
end
