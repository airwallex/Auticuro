import React from 'react';
import clsx from 'clsx';
import styles from './styles.module.css';

const FeatureList = [
  {
    title: 'Professional Financial Management',
    Svg: require('@site/static/img/index_page/management.svg').default,
    description: (
        <p style={{whiteSpace:"pre-wrap"}}>{`Support all asset classes
Composable balance operations
Easy integration with ERP`
        }</p>
    ),
  },
  {
    title: 'Proven Reliability',
    Svg: require('@site/static/img/index_page/reliability.svg').default,
    description: (
      <p style={{whiteSpace:"pre-wrap"}}>{`Blockchain-ed transaction record
Advanced consensus algorithm
Cloud native architecture`
      }</p>
    ),
  },
  {
    title: 'Performance',
    Svg: require('@site/static/img/index_page/fast.svg').default,
    description: (
        <p style={{whiteSpace:"pre-wrap"}}>{`Unlimited scalability
10,000 TPS for single account
10 millisecond latency (5K TPS)`
        }</p>
    ),
  },
];

function Feature({Svg, title, description}) {
  return (
    <div className={clsx('col col--4')}>
      <div className="text--center">
        <Svg className={styles.featureSvg} role="img" />
      </div>
      <div className="text--center padding-horiz--md">
        <h3>{title}</h3>
        <p>{description}</p>
      </div>
    </div>
  );
}

export default function HomepageFeatures() {
  return (
    <section className={styles.features}>
      <div className="container">
        <div className="row">
          {FeatureList.map((props, idx) => (
            <Feature key={idx} {...props} />
          ))}
        </div>
      </div>
    </section>
  );
}
