/*
 * ferens_irq - interrupt bridge for the ferens harness.
 *
 * Binds to a Polaris PCI device, allocates one MSI vector and turns every
 * GPU interrupt into a wake-up on a character device waitqueue. Userspace
 * keeps full control of MMIO and the IH ring through its own BAR mappings;
 * the module only delivers the signal, like uio_pci_generic does with INTx
 * but via MSI, which is what the IH of Polaris raises natively.
 *
 * /dev/ferens_irq0 ioctls:
 *   FERENS_IOC_WAIT(_IOW 'f' 1, u64 timeout_ms) - block until an interrupt
 *     arrives after the call, 0 means 5000 ms, cap 60000 ms.
 *   FERENS_IOC_COUNT(_IOR 'f' 2, u64 out) - interrupt count so far.
 *   FERENS_IOC_INFO(_IOR 'f' 3, struct ferens_info) - count, irq, msi flag,
 *     PCI identity.
 *   FERENS_IOC_RESET(_IO 'f' 4) - reset the counter.
 *
 * One device per module instance: enough for the harness, keeps the state
 * global-free from races on the second card.
 */

#include <linux/atomic.h>
#include <linux/cdev.h>
#include <linux/device.h>
#include <linux/errno.h>
#include <linux/fs.h>
#include <linux/interrupt.h>
#include <linux/ioctl.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/pci.h>
#include <linux/slab.h>
#include <linux/uaccess.h>
#include <linux/version.h>
#include <linux/wait.h>

#define FERENS_IRQ_NAME "ferens_irq"
#define FERENS_NODE_NAME "ferens_irq0"
#define FERENS_IOC_MAGIC 'f'
#define FERENS_DEFAULT_TIMEOUT_MS 5000ULL
#define FERENS_MAX_TIMEOUT_MS 60000ULL

struct ferens_info {
	__u64 count;
	__u32 irq;
	__u32 msi;
	__u16 vendor;
	__u16 device;
	__u16 pad[2];
};

#define FERENS_IOC_WAIT _IOW(FERENS_IOC_MAGIC, 1, __u64)
#define FERENS_IOC_COUNT _IOR(FERENS_IOC_MAGIC, 2, __u64)
#define FERENS_IOC_INFO _IOR(FERENS_IOC_MAGIC, 3, struct ferens_info)
#define FERENS_IOC_RESET _IO(FERENS_IOC_MAGIC, 4)

struct ferens_dev {
	struct pci_dev *pdev;
	int irq;
	atomic64_t count;
	wait_queue_head_t waitq;
};

static struct ferens_dev *ferens_device;
static DEFINE_MUTEX(ferens_state_lock);

static dev_t ferens_devt;
static struct cdev ferens_cdev;
static struct class *ferens_class;
static bool ferens_cdev_ready;

static const struct pci_device_id ferens_pci_ids[] = {
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67C0) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67C1) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67C2) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67C4) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67C7) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67C8) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67C9) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67CA) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67CC) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67CF) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67D0) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67D4) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67DF) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x6FDF) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67E0) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67E1) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67E3) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67E7) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67E8) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67E9) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67EB) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67EF) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x67FF) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x6980) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x6981) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x6985) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x6986) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x6987) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x698F) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x6995) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x6997) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x699F) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x694E) },
	{ PCI_DEVICE(PCI_VENDOR_ID_ATI, 0x694F) },
	{ }
};

MODULE_DEVICE_TABLE(pci, ferens_pci_ids);

static irqreturn_t ferens_irq_handler(int irq, void *data)
{
	struct ferens_dev *fdev = data;

	atomic64_inc(&fdev->count);
	wake_up_interruptible(&fdev->waitq);
	return IRQ_HANDLED;
}

static int ferens_pci_probe(struct pci_dev *pdev, const struct pci_device_id *id)
{
	struct ferens_dev *fdev;
	int vectors;
	int ret;

	mutex_lock(&ferens_state_lock);
	if (ferens_device) {
		mutex_unlock(&ferens_state_lock);
		dev_warn(&pdev->dev,
			 "ferens_irq already owns %s, one device per module\n",
			 pci_name(ferens_device->pdev));
		return -EBUSY;
	}
	mutex_unlock(&ferens_state_lock);

	fdev = devm_kzalloc(&pdev->dev, sizeof(*fdev), GFP_KERNEL);
	if (!fdev)
		return -ENOMEM;

	fdev->pdev = pdev;
	atomic64_set(&fdev->count, 0);
	init_waitqueue_head(&fdev->waitq);

	ret = pci_enable_device(pdev);
	if (ret)
		return ret;

	pci_set_master(pdev);

	vectors = pci_alloc_irq_vectors(pdev, 1, 1, PCI_IRQ_MSI);
	if (vectors < 1) {
		dev_err(&pdev->dev, "ferens_irq: no MSI vector, err %d\n", vectors);
		ret = vectors < 0 ? vectors : -ENODEV;
		goto out_disable;
	}

	fdev->irq = pci_irq_vector(pdev, 0);
	ret = request_irq(fdev->irq, ferens_irq_handler, 0, FERENS_IRQ_NAME, fdev);
	if (ret)
		goto out_free_vectors;

	mutex_lock(&ferens_state_lock);
	ferens_device = fdev;
	mutex_unlock(&ferens_state_lock);
	pci_set_drvdata(pdev, fdev);

	dev_info(&pdev->dev,
		 "ferens_irq bound: %04x:%04x, msi vector %d\n",
		 pdev->vendor, pdev->device, fdev->irq);
	return 0;

out_free_vectors:
	pci_free_irq_vectors(pdev);
out_disable:
	pci_disable_device(pdev);
	return ret;
}

static void ferens_pci_remove(struct pci_dev *pdev)
{
	struct ferens_dev *fdev = pci_get_drvdata(pdev);

	if (!fdev)
		return;

	free_irq(fdev->irq, fdev);
	pci_free_irq_vectors(pdev);
	pci_disable_device(pdev);

	mutex_lock(&ferens_state_lock);
	if (ferens_device == fdev)
		ferens_device = NULL;
	mutex_unlock(&ferens_state_lock);

	dev_info(&pdev->dev, "ferens_irq released, %llu interrupts delivered\n",
		 atomic64_read(&fdev->count));
}

static long ferens_ioctl_wait(unsigned long arg)
{
	struct ferens_dev *fdev;
	__u64 timeout_ms;
	__u64 before;
	long left;

	if (copy_from_user(&timeout_ms, (void __user *)arg, sizeof(timeout_ms)))
		return -EFAULT;
	if (timeout_ms == 0)
		timeout_ms = FERENS_DEFAULT_TIMEOUT_MS;
	if (timeout_ms > FERENS_MAX_TIMEOUT_MS)
		timeout_ms = FERENS_MAX_TIMEOUT_MS;

	mutex_lock(&ferens_state_lock);
	fdev = ferens_device;
	mutex_unlock(&ferens_state_lock);
	if (!fdev)
		return -ENODEV;

	before = atomic64_read(&fdev->count);
	left = wait_event_interruptible_timeout(fdev->waitq,
			atomic64_read(&fdev->count) != before,
			msecs_to_jiffies(timeout_ms));
	if (left > 0)
		return 0;
	if (left == 0)
		return -ETIMEDOUT;
	return -EINTR;
}

static long ferens_unlocked_ioctl(struct file *file, unsigned int cmd,
				  unsigned long arg)
{
	struct ferens_dev *fdev;

	switch (cmd) {
	case FERENS_IOC_WAIT:
		return ferens_ioctl_wait(arg);

	case FERENS_IOC_COUNT: {
		__u64 count;
		struct pci_dev *owner;

		mutex_lock(&ferens_state_lock);
		fdev = ferens_device;
		owner = fdev ? fdev->pdev : NULL;
		count = fdev ? atomic64_read(&fdev->count) : 0;
		mutex_unlock(&ferens_state_lock);
		if (!owner)
			return -ENODEV;
		return copy_to_user((void __user *)arg, &count, sizeof(count)) ?
			       -EFAULT : 0;
	}

	case FERENS_IOC_INFO: {
		struct ferens_info info;
		struct pci_dev *owner;

		mutex_lock(&ferens_state_lock);
		fdev = ferens_device;
		owner = fdev ? fdev->pdev : NULL;
		if (fdev) {
			info.count = atomic64_read(&fdev->count);
			info.irq = (__u32)fdev->irq;
			info.msi = pci_dev_msi_enabled(fdev->pdev) ? 1u : 0u;
			info.vendor = fdev->pdev->vendor;
			info.device = fdev->pdev->device;
			info.pad[0] = 0;
			info.pad[1] = 0;
		}
		mutex_unlock(&ferens_state_lock);
		if (!owner)
			return -ENODEV;
		return copy_to_user((void __user *)arg, &info, sizeof(info)) ?
			       -EFAULT : 0;
	}

	case FERENS_IOC_RESET: {
		mutex_lock(&ferens_state_lock);
		fdev = ferens_device;
		if (fdev)
			atomic64_set(&fdev->count, 0);
		mutex_unlock(&ferens_state_lock);
		return fdev ? 0 : -ENODEV;
	}

	default:
		return -ENOTTY;
	}
}

static const struct file_operations ferens_fops = {
	.owner = THIS_MODULE,
	.unlocked_ioctl = ferens_unlocked_ioctl,
#ifdef CONFIG_COMPAT
	.compat_ioctl = ferens_unlocked_ioctl,
#endif
	.llseek = no_llseek,
};

static int __init ferens_chardev_init(void)
{
	int ret;

	ret = alloc_chrdev_region(&ferens_devt, 0, 1, FERENS_IRQ_NAME);
	if (ret)
		return ret;

	cdev_init(&ferens_cdev, &ferens_fops);
	ferens_cdev.owner = THIS_MODULE;
	ret = cdev_add(&ferens_cdev, ferens_devt, 1);
	if (ret)
		goto out_unregister;

#if LINUX_VERSION_CODE >= KERNEL_VERSION(6, 4, 0)
	ferens_class = class_create(FERENS_IRQ_NAME);
#else
	ferens_class = class_create(THIS_MODULE, FERENS_IRQ_NAME);
#endif
	if (IS_ERR(ferens_class)) {
		ret = PTR_ERR(ferens_class);
		goto out_del;
	}

	device_create(ferens_class, NULL, ferens_devt, NULL, FERENS_NODE_NAME);
	ferens_cdev_ready = true;
	return 0;

out_del:
	cdev_del(&ferens_cdev);
out_unregister:
	unregister_chrdev_region(ferens_devt, 1);
	return ret;
}

static void __exit ferens_chardev_exit(void)
{
	if (ferens_cdev_ready) {
		device_destroy(ferens_class, ferens_devt);
		class_destroy(ferens_class);
		cdev_del(&ferens_cdev);
		unregister_chrdev_region(ferens_devt, 1);
	}
}

static struct pci_driver ferens_pci_driver = {
	.name = FERENS_IRQ_NAME,
	.id_table = ferens_pci_ids,
	.probe = ferens_pci_probe,
	.remove = ferens_pci_remove,
};

static int __init ferens_init(void)
{
	int ret;

	ret = ferens_chardev_init();
	if (ret)
		return ret;

	ret = pci_register_driver(&ferens_pci_driver);
	if (ret)
		goto out_chardev;

	return 0;

out_chardev:
	ferens_chardev_exit();
	return ret;
}

static void __exit ferens_exit(void)
{
	pci_unregister_driver(&ferens_pci_driver);
	ferens_chardev_exit();
}

module_init(ferens_init);
module_exit(ferens_exit);

MODULE_LICENSE("GPL");
MODULE_AUTHOR("mel0k1");
MODULE_DESCRIPTION("ferens interrupt bridge: one MSI vector to /dev/ferens_irq0");
MODULE_VERSION("0.1");
